//! KCAN-over-TCP adapter with X25519 + AES-256-GCM session encryption.
//!
//! Scan the device's e-paper QR code to obtain the K1 URI
//! (`K1:<8-hex-ip>/<43-base64url-pubkey>`), then pass it to [`KCanNetAdapter::open`].
//! The adapter parses the URI, dials TCP port 3333, and performs the ECDH
//! handshake before exchanging 108-byte
//! [`EncryptedKCanFrame`]s for the life of the connection.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use host_can::frame::CanFrame;
use kcan_protocol::frame::{FrameFlags, FrameType, KCanFrame};
use kcan_protocol::{
    EncryptedKCanFrame, EncryptionLayer, SoftwareEncryptionLayer, ENCRYPTED_FRAME_SIZE,
};

use super::{AdapterError, CanAdapter, ReceivedFrame};

const NET_PORT: u16 = 3333;

// ─── Public adapter ───────────────────────────────────────────────────────────

pub struct KCanNetAdapter {
    /// Shared with the reader thread; each acquisition is brief (one AES-GCM op).
    layer: Arc<Mutex<SoftwareEncryptionLayer>>,
    write_stream: Arc<Mutex<TcpStream>>,
    frame_rx: mpsc::Receiver<KCanFrame>,
    error_rx: mpsc::Receiver<String>,
    reader_thread: Option<std::thread::JoinHandle<()>>,
    name: String,
    tx_seq: u16,
}

impl KCanNetAdapter {
    /// Parse `uri`, connect (direct or via kgate relay), perform the ECDH handshake,
    /// and start the reader thread.
    pub fn open(uri: &str) -> Result<Self, AdapterError> {
        let k1 = parse_k1_uri(uri)
            .ok_or_else(|| AdapterError::Protocol(format!("bad K1 URI: {uri}")))?;

        let (stream, device_pk, name) = match k1 {
            K1Uri::Lan { ip, device_pk } => {
                let addr = std::net::SocketAddr::from((std::net::Ipv4Addr::from(ip), NET_PORT));
                let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
                    .map_err(|e| AdapterError::Io(format!("connect {addr}: {e}")))?;
                stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
                stream.set_write_timeout(Some(Duration::from_secs(10))).ok();
                let name = format!("KCanNet {}:{NET_PORT}", std::net::Ipv4Addr::from(ip));
                (stream, device_pk, name)
            }
            K1Uri::Relay { room_id, device_pk } => {
                // Resolve and connect — try every resolved address so an IPv6-first
                // result doesn't silently fail when an IPv4 address would succeed.
                let addrs: Vec<_> = "kgate.kodezine.com:4444"
                    .to_socket_addrs()
                    .map_err(|e| AdapterError::Io(format!("relay DNS: {e}")))?
                    .collect();
                if addrs.is_empty() {
                    return Err(AdapterError::Protocol("relay DNS: no addresses".into()));
                }
                let mut last_err = String::new();
                let mut connected = None;
                for addr in &addrs {
                    match TcpStream::connect_timeout(addr, Duration::from_secs(5)) {
                        Ok(s) => {
                            connected = Some(s);
                            break;
                        }
                        Err(e) => last_err = e.to_string(),
                    }
                }
                let mut stream = connected
                    .ok_or_else(|| AdapterError::Io(format!("connect relay: {last_err}")))?;
                stream.set_write_timeout(Some(Duration::from_secs(10))).ok();
                // 30s covers kgate pairing + ECDH over relay; device should already be
                // registered (it connects at boot), so typical latency is <1s.
                stream.set_read_timeout(Some(Duration::from_secs(30))).ok();
                // Announce room_id to kgate — triggers pairing with the waiting device.
                write_exact(&mut stream, &room_id)?;
                (
                    stream,
                    device_pk,
                    "KCanNet relay kgate.kodezine.com:4444".into(),
                )
            }
        };

        // Generate host ephemeral keypair from OS entropy (host = initiator, prefix=1).
        let mut entropy = [0u8; 32];
        getrandom::fill(&mut entropy).map_err(|e| AdapterError::Io(format!("rng: {e}")))?;
        let mut layer = SoftwareEncryptionLayer::from_entropy(entropy, true);
        let our_pk = layer.our_public_key();

        // Handshake: host sends its pubkey, device echoes its pubkey for MITM verification.
        //   1. Host → Device: 32B host ephemeral pubkey
        //   2. Device → Host: 32B device pubkey (host verifies vs K1 URI)
        {
            let mut s = stream
                .try_clone()
                .map_err(|e| AdapterError::Io(e.to_string()))?;
            write_exact(&mut s, &our_pk)?;
            let mut dev_pk_recv = [0u8; 32];
            read_exact(&mut s, &mut dev_pk_recv)?;
            if dev_pk_recv != device_pk {
                return Err(AdapterError::Protocol(
                    "device pubkey from handshake does not match K1 URI — possible MITM".into(),
                ));
            }
        }

        // Both sides now derive the same shared secret.
        layer
            .establish_session(&device_pk)
            .map_err(|_| AdapterError::Protocol("ECDH establish_session failed".into()))?;

        let layer = Arc::new(Mutex::new(layer));
        let write_stream = Arc::new(Mutex::new(
            stream
                .try_clone()
                .map_err(|e| AdapterError::Io(e.to_string()))?,
        ));

        let (frame_tx, frame_rx) = mpsc::channel::<KCanFrame>();
        let (error_tx, error_rx) = mpsc::sync_channel::<String>(1);

        let layer_r = Arc::clone(&layer);
        let reader_handle = std::thread::Builder::new()
            .name("kcannet-reader".into())
            .spawn(move || net_reader(stream, layer_r, frame_tx, error_tx))
            .map_err(|e| AdapterError::Io(format!("spawn reader: {e}")))?;

        Ok(Self {
            layer,
            write_stream,
            frame_rx,
            error_rx,
            reader_thread: Some(reader_handle),
            name,
            tx_seq: 0,
        })
    }

    fn next_seq(&mut self) -> u16 {
        let s = self.tx_seq;
        self.tx_seq = self.tx_seq.wrapping_add(1);
        s
    }

    /// Quick reachability check: TCP connect with a short timeout.
    pub fn probe(uri: &str) -> bool {
        match parse_k1_uri(uri) {
            Some(K1Uri::Lan { ip, .. }) => {
                let addr = std::net::SocketAddr::from((std::net::Ipv4Addr::from(ip), NET_PORT));
                TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_ok()
            }
            Some(K1Uri::Relay { .. }) => {
                // Try all resolved addresses, same as open().
                "kgate.kodezine.com:4444"
                    .to_socket_addrs()
                    .ok()
                    .map(|mut addrs| {
                        addrs.any(|addr| {
                            TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_ok()
                        })
                    })
                    .unwrap_or(false)
            }
            None => false,
        }
    }
}

impl Drop for KCanNetAdapter {
    fn drop(&mut self) {
        // Wake the blocked reader by shutting the socket, then join.
        if let Ok(s) = self.write_stream.lock() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        if let Some(h) = self.reader_thread.take() {
            let _ = h.join();
        }
    }
}

impl CanAdapter for KCanNetAdapter {
    fn recv(&mut self, timeout: Duration) -> Result<ReceivedFrame, AdapterError> {
        loop {
            match self.frame_rx.recv_timeout(timeout) {
                Ok(kf) => {
                    let is_tx_echo = kf.frame_type == FrameType::TxEcho as u8;
                    if kf.frame_type != FrameType::Data as u8 && !is_tx_echo {
                        continue; // skip Status/BusError
                    }
                    let frame = kcan_to_can_frame(&kf)
                        .ok_or_else(|| AdapterError::Protocol("invalid CAN ID".into()))?;
                    return Ok(ReceivedFrame {
                        frame,
                        hardware_timestamp_ns: Some(kf.timestamp_100ns as u64 * 100),
                        channel: kf.channel,
                        is_tx_echo,
                    });
                }
                Err(mpsc::RecvTimeoutError::Timeout) => return Err(AdapterError::Timeout),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let reason = self.error_rx.try_recv().unwrap_or_default();
                    if !reason.is_empty() {
                        eprintln!("kcannet reader died: {reason}");
                    }
                    return Err(AdapterError::Disconnected);
                }
            }
        }
    }

    fn send(&mut self, frame: &CanFrame) -> Result<(), AdapterError> {
        let seq = self.next_seq();
        let kf = can_frame_to_kcan(frame, seq);
        let enc = self
            .layer
            .lock()
            .unwrap()
            .encrypt_frame(&kf.to_bytes())
            .map_err(|_| AdapterError::Io("encrypt failed".into()))?;
        let mut s = self.write_stream.lock().unwrap();
        write_exact(&mut s, &enc.to_bytes())
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn echoes_tx(&self) -> bool {
        true
    }
}

// ─── Background reader ────────────────────────────────────────────────────────

fn net_reader(
    mut stream: TcpStream,
    layer: Arc<Mutex<SoftwareEncryptionLayer>>,
    frame_tx: mpsc::Sender<KCanFrame>,
    error_tx: mpsc::SyncSender<String>,
) {
    // Short read timeout so the thread can be woken by Drop's shutdown() call.
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .ok();
    let mut in_buf = [0u8; ENCRYPTED_FRAME_SIZE];
    let mut in_off = 0usize;
    loop {
        match stream.read(&mut in_buf[in_off..]) {
            Ok(0) => {
                error_tx.try_send("connection closed".into()).ok();
                return;
            }
            Ok(n) => {
                in_off += n;
                if in_off == ENCRYPTED_FRAME_SIZE {
                    let enc = EncryptedKCanFrame::from_bytes(&in_buf);
                    match layer.lock().unwrap().decrypt_frame(&enc) {
                        Ok(plain) => {
                            if let Some(f) = KCanFrame::from_bytes(&plain) {
                                if frame_tx.send(f).is_err() {
                                    return;
                                }
                            }
                        }
                        Err(_) => {
                            eprintln!("kcannet: GCM authentication failed — frame discarded")
                        }
                    }
                    in_off = 0;
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(e) => {
                error_tx.try_send(format!("read error: {e}")).ok();
                return;
            }
        }
    }
}

// ─── K1 URI parsing ───────────────────────────────────────────────────────────

enum K1Uri {
    /// `K1:<8-hex-ip>/<43-b64url-pubkey>` — direct TCP to device IP.
    Lan { ip: [u8; 4], device_pk: [u8; 32] },
    /// `K1:r/<6-char-room-id>/<43-b64url-pubkey>` — routed via kgate relay.
    Relay {
        room_id: [u8; 6],
        device_pk: [u8; 32],
    },
}

/// Parse a K1 URI in either LAN or relay format.
///
/// Both formats have identical byte length after stripping `K1:` (52 chars);
/// the discriminator is whether the string starts with `r/`.
fn parse_k1_uri(uri: &str) -> Option<K1Uri> {
    let rest = uri.trim().strip_prefix("K1:")?;
    if rest.len() != 52 {
        return None;
    }
    if let Some(relay_rest) = rest.strip_prefix("r/") {
        // Relay: `r/<6-char-room-id>/<43-char-pubkey>` — rest after "r/" = 50 chars
        if relay_rest.as_bytes().get(6) != Some(&b'/') {
            return None;
        }
        let room_id: [u8; 6] = relay_rest.as_bytes()[..6].try_into().ok()?;
        let pk = decode_base64url_32(&relay_rest.as_bytes()[7..50])?;
        Some(K1Uri::Relay {
            room_id,
            device_pk: pk,
        })
    } else {
        // LAN: `<8-hex-ip>/<43-char-pubkey>`
        if rest.as_bytes()[8] != b'/' {
            return None;
        }
        let ip = parse_hex_ip(&rest[..8])?;
        let pk = decode_base64url_32(&rest.as_bytes()[9..52])?;
        Some(K1Uri::Lan { ip, device_pk: pk })
    }
}

fn parse_hex_ip(s: &str) -> Option<[u8; 4]> {
    if s.len() != 8 {
        return None;
    }
    let b = s.as_bytes();
    let mut ip = [0u8; 4];
    for i in 0..4 {
        ip[i] = (hex_val(b[i * 2])? << 4) | hex_val(b[i * 2 + 1])?;
    }
    Some(ip)
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

/// Decode exactly 43 base64url (no padding) characters → 32 bytes.
///
/// 32 bytes = 256 bits; 43 × 6 = 258 bits, so the last two bits are padding
/// zeros.  Layout: 10 full groups of 4 chars → 30 bytes, then 3 chars → 2 bytes.
fn decode_base64url_32(input: &[u8]) -> Option<[u8; 32]> {
    if input.len() != 43 {
        return None;
    }
    let mut out = [0u8; 32];
    let mut oi = 0usize;
    // 10 × 4-char groups → 30 bytes
    for i in (0..40).step_by(4) {
        let (a, b, c, d) = (
            b64v(input[i])?,
            b64v(input[i + 1])?,
            b64v(input[i + 2])?,
            b64v(input[i + 3])?,
        );
        let v = ((a as u32) << 18) | ((b as u32) << 12) | ((c as u32) << 6) | (d as u32);
        out[oi] = (v >> 16) as u8;
        out[oi + 1] = (v >> 8) as u8;
        out[oi + 2] = v as u8;
        oi += 3;
    }
    // 3 trailing chars → 2 bytes (last 2 bits are padding zeros)
    let (a, b, c) = (b64v(input[40])?, b64v(input[41])?, b64v(input[42])?);
    let v = ((a as u32) << 18) | ((b as u32) << 12) | ((c as u32) << 6);
    out[30] = (v >> 16) as u8;
    out[31] = (v >> 8) as u8;
    Some(out)
}

fn b64v(b: u8) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}

// ─── Frame conversion (mirrors kcan/mod.rs) ───────────────────────────────────

fn kcan_to_can_frame(kf: &KCanFrame) -> Option<CanFrame> {
    use embedded_can::{ExtendedId, Frame, Id, StandardId};
    let dlc = kf.dlc as usize;
    let data = &kf.data[..dlc.min(8)];
    let is_eff = kf.flags & FrameFlags::EFF != 0;
    let is_rtr = kf.flags & FrameFlags::RTR != 0;
    let id: Id = if is_eff {
        Id::Extended(ExtendedId::new(kf.can_id & 0x1FFF_FFFF)?)
    } else {
        Id::Standard(StandardId::new((kf.can_id & 0x7FF) as u16)?)
    };
    if is_rtr {
        CanFrame::new_remote(id, dlc)
    } else {
        CanFrame::new(id, data)
    }
}

fn can_frame_to_kcan(frame: &CanFrame, seq: u16) -> KCanFrame {
    use embedded_can::{Frame, Id};
    let mut flags: u8 = 0;
    let can_id: u32;
    match frame.id() {
        Id::Standard(id) => {
            can_id = id.as_raw() as u32;
        }
        Id::Extended(id) => {
            can_id = id.as_raw();
            flags |= FrameFlags::EFF;
        }
    }
    if frame.is_remote_frame() {
        flags |= FrameFlags::RTR;
    }
    let data = frame.data();
    KCanFrame::new_tx(can_id, flags, data.len() as u8, data, seq)
}

// ─── I/O helpers ──────────────────────────────────────────────────────────────

fn write_exact(s: &mut TcpStream, buf: &[u8]) -> Result<(), AdapterError> {
    s.write_all(buf)
        .map_err(|e| AdapterError::Io(format!("write: {e}")))
}

fn read_exact(s: &mut TcpStream, buf: &mut [u8]) -> Result<(), AdapterError> {
    s.read_exact(buf)
        .map_err(|e| AdapterError::Io(format!("read: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Known-good URI from bench session: 192.168.3.178 + real pubkey bytes.
    const VALID_URI: &str = "K1:C0A803B2/yhRv_Y28dyfXdvgJG4V5XeGNv4NqbVjJqqvy6Y2seig";

    #[test]
    fn parse_valid_uri() {
        let K1Uri::Lan { ip, .. } = parse_k1_uri(VALID_URI).expect("should parse") else {
            panic!("expected Lan variant");
        };
        assert_eq!(ip, [192, 168, 3, 178]);
    }

    #[test]
    fn parse_valid_relay_uri() {
        let relay_uri = "K1:r/10CDVa/WoLHNmyWW1OlVT3KoOMS9CTdBM6ILy6Q6ywyxZesnW0";
        let K1Uri::Relay { room_id, .. } = parse_k1_uri(relay_uri).expect("relay should parse")
        else {
            panic!("expected Relay variant");
        };
        assert_eq!(&room_id, b"10CDVa");
    }

    #[test]
    fn parse_trims_whitespace() {
        let uri = format!("  {VALID_URI}\n");
        assert!(parse_k1_uri(&uri).is_some());
    }

    #[test]
    fn parse_rejects_wrong_prefix() {
        assert!(parse_k1_uri("K2:C0A803B2/yhRv_Y28dyfXdvgJG4V5XeGNv4NqbVjJqqvy6Y2seig").is_none());
    }

    #[test]
    fn parse_rejects_missing_slash() {
        // Replace the '/' separator with 'X'.
        let bad = VALID_URI.replacen('/', "X", 1);
        assert!(parse_k1_uri(&bad).is_none());
    }

    #[test]
    fn parse_rejects_bad_hex_ip() {
        // 'GG' is not valid hex.
        assert!(parse_k1_uri("K1:GGH803B2/yhRv_Y28dyfXdvgJG4V5XeGNv4NqbVjJqqvy6Y2seig").is_none());
    }

    #[test]
    fn parse_rejects_short_pubkey() {
        // Truncate the base64url section.
        assert!(parse_k1_uri("K1:C0A803B2/yhRv_Y28dyfXdvgJG4V5XeGN").is_none());
    }

    #[test]
    fn parse_rejects_bad_base64_char() {
        // '!' is not a valid base64url character.
        let bad = VALID_URI.replacen('y', "!", 1);
        assert!(parse_k1_uri(&bad).is_none());
    }

    #[test]
    fn decode_base64url_32_roundtrips() {
        let original: [u8; 32] = core::array::from_fn(|i| i as u8);
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut encoded = [0u8; 43];
        let mut oi = 0;
        for chunk in original.chunks(3) {
            let (a, b, c) = (
                chunk[0],
                chunk.get(1).copied().unwrap_or(0),
                chunk.get(2).copied().unwrap_or(0),
            );
            let v24 = ((a as u32) << 16) | ((b as u32) << 8) | (c as u32);
            for shift in [18u32, 12, 6, 0] {
                if oi < 43 {
                    encoded[oi] = A[((v24 >> shift) & 63) as usize];
                    oi += 1;
                }
            }
        }
        let decoded = decode_base64url_32(&encoded).unwrap();
        assert_eq!(decoded, original);
    }
}
