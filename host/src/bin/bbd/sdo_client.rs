//! Synchronous CANopen SDO client for the `bbd` firmware download tool.
//!
//! Wraps a [`CanAdapter`] and provides blocking SDO read/write operations that
//! the firmware download state machine relies on. There is no async runtime —
//! every call blocks the calling thread until a response arrives or the
//! configured timeout elapses.
//!
//! # SDO base IDs
//! - Request  (master → node): COB-ID = `tx_base_id` + `node_id`  (default 0x600 + node_id)
//! - Response (node → master): COB-ID = `rx_base_id` + `node_id`  (default 0x580 + node_id)

use std::time::{Duration, Instant};

use embedded_can::{Frame as EmbeddedFrame, Id, StandardId};
use host_can::frame::CanFrame;

use rustycan::adapters::{AdapterError, CanAdapter};
use rustycan::canopen::sdo::{
    calculate_crc16, decode_block_download_end_response, decode_block_download_initiate_response,
    decode_block_download_subblock_response, decode_segmented_upload_initiate,
    decode_upload_segment_response, encode_abort, encode_block_download_end,
    encode_block_download_initiate, encode_download_expedited, encode_download_initiate_segmented,
    encode_download_segment, encode_upload_request, encode_upload_segment_ack,
    is_download_initiate_ack, is_download_segment_ack,
};

// ─── Constants ───────────────────────────────────────────────────────────────

/// Default number of segments per block for block-mode SDO downloads.
const DEFAULT_BLOCK_SIZE: u8 = 16;

/// Abort code a CANopen server (or bbd's own startup `send_abort`) emits for
/// "SDO protocol timed out". A stale copy for the object being transacted on can
/// linger in the Apex device RX FIFO after an interrupted download (see
/// kodezine/RustyCAN#107); the initiating poll loop treats it as stale and
/// re-sends rather than surfacing it.
const SDO_ABORT_PROTOCOL_TIMEOUT: u32 = 0x0504_0000;

/// Re-send cadence for the initiating exchange of an idempotent SDO
/// transaction. Re-issuing the request is what shakes a response the Apex
/// device is withholding out of its RX FIFO (#107); polling on this interval
/// mirrors the flash-status loop that already punches through.
const INITIATE_RESEND_INTERVAL: Duration = Duration::from_millis(250);

/// Bootloader device type value in CANopen object 0x1000 subindex 0.
pub const BOOTLOADER_DEVICE_TYPE: u32 = 0x1000_0000;
/// Bootloader-update-app device type (loaded via `--blupdate-app`).
pub const BLUPDATE_APP_DEVICE_TYPE: u32 = 0x2000_0000;

// ─── Error type ──────────────────────────────────────────────────────────────

/// Errors that the SDO client can produce.
#[derive(Debug)]
pub enum SdoError {
    /// No response received within the configured timeout.
    Timeout,
    /// The node returned an SDO abort with this abort code.
    Abort(u32),
    /// The adapter returned a hard error.
    Adapter(AdapterError),
    /// The server returned an unexpected/malformed response.
    Protocol(String),
}

impl std::fmt::Display for SdoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "SDO timeout (no response from node)"),
            Self::Abort(code) => write!(f, "SDO abort 0x{code:08X}"),
            Self::Adapter(e) => write!(f, "adapter error: {e}"),
            Self::Protocol(s) => write!(f, "protocol error: {s}"),
        }
    }
}

// ─── SDO transfer mode ───────────────────────────────────────────────────────

/// Which SDO transfer mechanism to use for large (>4-byte) downloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdocType {
    /// Segmented download — compatible with all CANopen bootloaders.
    Segmented = 0,
    /// Block download — higher throughput for large payloads.
    Block = 2,
    /// Segmented download that streams every segment without reading the
    /// per-segment acks. For adapters that drop SDO responses (Apex, #107):
    /// correctness is confirmed by the caller's flash-status poll, not the acks.
    SegmentedNoWait = 3,
}

impl SdocType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Segmented),
            2 => Some(Self::Block),
            3 => Some(Self::SegmentedNoWait),
            _ => None,
        }
    }
}

// ─── SDO client ──────────────────────────────────────────────────────────────

/// Configuration for the SDO client.
pub struct SdoClientConfig {
    /// Target CANopen node ID (1–127).
    pub node_id: u8,
    /// Timeout for each individual SDO exchange.
    pub timeout: Duration,
    /// COB-ID base for requests (master → node). Default 0x600.
    pub tx_base_id: u16,
    /// COB-ID base for responses (node → master). Default 0x580.
    pub rx_base_id: u16,
}

impl Default for SdoClientConfig {
    fn default() -> Self {
        Self {
            node_id: 1,
            timeout: Duration::from_millis(500),
            tx_base_id: 0x600,
            rx_base_id: 0x580,
        }
    }
}

/// Blocking CANopen SDO master client.
pub struct SdoClient {
    adapter: Box<dyn CanAdapter>,
    cfg: SdoClientConfig,
}

impl SdoClient {
    pub fn new(adapter: Box<dyn CanAdapter>, cfg: SdoClientConfig) -> Self {
        Self { adapter, cfg }
    }

    // ── helpers ──────────────────────────────────────────────────────────────

    fn request_cob_id(&self) -> u16 {
        self.cfg.tx_base_id + self.cfg.node_id as u16
    }

    fn response_cob_id(&self) -> u16 {
        self.cfg.rx_base_id + self.cfg.node_id as u16
    }

    /// Build a CAN frame addressed to the node (SDO request).
    fn make_request_frame(&self, data: [u8; 8]) -> CanFrame {
        let id = StandardId::new(self.request_cob_id()).expect("request COB-ID out of range");
        CanFrame::new(Id::Standard(id), &data).expect("frame construction failed")
    }

    /// Send a raw 8-byte SDO request frame.
    fn send(&mut self, data: [u8; 8]) -> Result<(), SdoError> {
        let frame = self.make_request_frame(data);
        self.adapter.send(&frame).map_err(SdoError::Adapter)
    }

    /// Discard any buffered frames left over from a previous transaction.
    ///
    /// `recv_response` matches replies only by COB-ID, so a late or duplicate
    /// response (for example a flash-status upload reply that arrived after its
    /// `read_u32` poll already timed out during `WaitClear`) can linger in the
    /// adapter's RX queue and then be mistaken for the reply to the *next*
    /// request. Call this immediately before sending the initiating request of
    /// a new logical SDO transaction so every request is answered by its own
    /// response. Any frame already queued at this point is by definition stale.
    fn drain_rx(&mut self) {
        // A short, non-accumulating poll: we only clear frames already buffered
        // — no new request has been sent yet, so nothing legitimate is inbound.
        while self.adapter.recv(Duration::from_millis(2)).is_ok() {
            // discard stale frame and keep draining until the queue is empty
        }
    }

    /// Wait for a CAN frame from the node's SDO response COB-ID.
    ///
    /// Frames from other COB-IDs are silently discarded. Returns the 8-byte
    /// data payload of the matching frame, or [`SdoError::Timeout`].
    fn recv_response(&mut self) -> Result<[u8; 8], SdoError> {
        // No multiplexer for bare segment/sub-block acks: surface any abort.
        self.recv_response_matching(None, |_| true)
    }

    /// Wait for an SDO response frame that satisfies `accept`.
    ///
    /// Frames from other COB-IDs are ignored, as are matching-COB-ID frames
    /// that `accept` rejects — those are treated as stale/late replies from a
    /// previous transaction (for example a flash-status upload response that
    /// arrived after its poll timed out during `WaitClear`) and skipped until
    /// the real reply arrives or the timeout elapses.
    ///
    /// An SDO abort (CS = 0x80) is surfaced only when it targets the object in
    /// `expect_mux` (index in bytes 1-2, subindex in byte 3). A stale abort for
    /// a *different* object — left in the adapter/device RX queue by a prior
    /// interrupted session (see kodezine/RustyCAN#107) — is skipped like any
    /// other stale reply. Pass `None` for transfers with no multiplexer
    /// (segment / sub-block / end acks), where any abort on the response COB-ID
    /// is surfaced. Malformed frames are always surfaced immediately.
    fn recv_response_matching<F>(
        &mut self,
        expect_mux: Option<(u8, u8, u8)>,
        accept: F,
    ) -> Result<[u8; 8], SdoError>
    where
        F: Fn(&[u8; 8]) -> bool,
    {
        self.recv_response_matching_within(self.cfg.timeout, expect_mux, accept)
    }

    /// As [`recv_response_matching`], but waits at most `timeout` for a match
    /// instead of the client's configured per-exchange timeout.
    fn recv_response_matching_within<F>(
        &mut self,
        timeout: Duration,
        expect_mux: Option<(u8, u8, u8)>,
        accept: F,
    ) -> Result<[u8; 8], SdoError>
    where
        F: Fn(&[u8; 8]) -> bool,
    {
        let expected_cob = self.response_cob_id();
        let deadline = Instant::now() + timeout;

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(SdoError::Timeout);
            }

            match self.adapter.recv(remaining) {
                Ok(rx) => {
                    let cob = match rx.frame.id() {
                        Id::Standard(sid) => sid.as_raw(),
                        Id::Extended(eid) => (eid.as_raw() & 0x7FF) as u16,
                    };
                    if cob != expected_cob {
                        continue; // not our response, keep waiting
                    }
                    let raw = rx.frame.data();
                    if raw.len() < 8 {
                        return Err(SdoError::Protocol(format!(
                            "response frame too short: {} bytes",
                            raw.len()
                        )));
                    }
                    // SDO abort (CS = 0x80). Only surface it when it targets the
                    // object being transacted on; a stale abort for a different
                    // object (left in the device RX FIFO by a prior session, see
                    // kodezine/RustyCAN#107) is skipped like any other stale reply.
                    if raw[0] == 0x80 {
                        if let Some((lo, hi, sub)) = expect_mux {
                            if raw[1] != lo || raw[2] != hi || raw[3] != sub {
                                continue;
                            }
                        }
                        let code = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
                        return Err(SdoError::Abort(code));
                    }
                    let mut out = [0u8; 8];
                    out.copy_from_slice(&raw[..8]);
                    if !accept(&out) {
                        // Stale/late reply from an earlier transaction — discard
                        // and keep waiting for the response we actually expect.
                        continue;
                    }
                    return Ok(out);
                }
                Err(AdapterError::Timeout) => {
                    if Instant::now() >= deadline {
                        return Err(SdoError::Timeout);
                    }
                    // Not yet expired — retry
                }
                Err(e) => return Err(SdoError::Adapter(e)),
            }
        }
    }

    /// Send an idempotent initiating SDO request and wait for its matching
    /// response, re-issuing the request on a short cadence until it is answered
    /// or the client's timeout budget is spent.
    ///
    /// Only for requests that may be replayed safely — an upload, or the
    /// initiate of a download before any segment/block data is sent. The RX is
    /// drained once up front to clear cross-session backlog; thereafter the
    /// request is re-sent every [`INITIATE_RESEND_INTERVAL`] without draining,
    /// so a response the Apex device released late (only after a subsequent TX,
    /// see kodezine/RustyCAN#107) is still accepted. A stale same-object
    /// "protocol timed out" abort left by a prior interrupted transfer is
    /// skipped like any other stale reply; every other abort is surfaced.
    fn initiate_retry<F>(
        &mut self,
        request: [u8; 8],
        expect_mux: Option<(u8, u8, u8)>,
        accept: F,
    ) -> Result<[u8; 8], SdoError>
    where
        F: Fn(&[u8; 8]) -> bool,
    {
        let deadline = Instant::now() + self.cfg.timeout;
        self.drain_rx();
        loop {
            self.send(request)?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            let per_attempt = INITIATE_RESEND_INTERVAL.min(remaining);
            match self.recv_response_matching_within(per_attempt, expect_mux, &accept) {
                Ok(resp) => return Ok(resp),
                Err(SdoError::Timeout) | Err(SdoError::Abort(SDO_ABORT_PROTOCOL_TIMEOUT))
                    if Instant::now() < deadline =>
                {
                    // No fresh answer yet (or a stale protocol-timeout abort
                    // for this object) — re-send and keep polling.
                }
                other => return other,
            }
        }
    }

    // ── Public SDO operations ─────────────────────────────────────────────────

    /// Read a 32-bit value from the node via SDO upload.
    ///
    /// Accepts both expedited (≤4-byte objects sent inline) and segmented
    /// responses. Some bootloaders respond with a segmented initiate even for
    /// UNSIGNED32 objects, so both paths are handled transparently.
    pub fn read_u32(&mut self, index: u16, subindex: u8) -> Result<u32, SdoError> {
        // Accept only an upload initiate response (SCS=2) for *this* object.
        // Besides the SCS bits, match the echoed multiplexer (index in bytes
        // 1-2, subindex in byte 3) so a stale reply for a different object on
        // the same COB-ID (e.g. a leftover 0xA2 block ack or 0x43 status reply
        // from a prior interrupted transfer) is skipped until the real reply
        // arrives or we time out.
        let [idx_lo, idx_hi] = index.to_le_bytes();
        let resp = self.initiate_retry(
            encode_upload_request(index, subindex),
            Some((idx_lo, idx_hi, subindex)),
            |r| r[0] & 0xE0 == 0x40 && r[1] == idx_lo && r[2] == idx_hi && r[3] == subindex,
        )?;
        let cs = resp[0];

        // Expedited: SCS=2 (bits 7-5 = 010), e=1 (bit 1 set)
        if cs & 0xE0 == 0x40 && cs & 0x02 != 0 {
            return Ok(u32::from_le_bytes([resp[4], resp[5], resp[6], resp[7]]));
        }

        // Segmented upload initiate: SCS=2, e=0
        if decode_segmented_upload_initiate(&resp).is_none() {
            return Err(SdoError::Protocol(format!(
                "expected upload response (expedited or segmented), got 0x{cs:02X}"
            )));
        }

        // Request the first (and for UINT32, only) segment.
        self.send(encode_upload_segment_ack(false))?;
        let seg = self.recv_response()?;
        let (payload, is_last) = decode_upload_segment_response(&seg).ok_or_else(|| {
            SdoError::Protocol(format!(
                "expected upload segment response, got 0x{:02X}",
                seg[0]
            ))
        })?;

        if payload.len() < 4 {
            return Err(SdoError::Protocol(format!(
                "segmented UINT32 too short: {} bytes",
                payload.len()
            )));
        }
        // If there are more segments (shouldn't happen for UINT32), abort cleanly.
        if !is_last {
            let _ = self.send_abort(index, subindex);
        }
        Ok(u32::from_le_bytes([
            payload[0], payload[1], payload[2], payload[3],
        ]))
    }

    /// Read a visible string object from the node via segmented SDO upload.
    ///
    /// Handles both expedited (short strings ≤4 bytes) and segmented responses.
    /// Returns the string with any trailing NUL bytes stripped.
    pub fn read_string(&mut self, index: u16, subindex: u8) -> Result<String, SdoError> {
        // Accept only an upload initiate response (SCS=2) for *this* object,
        // matching the echoed multiplexer (index in bytes 1-2, subindex in
        // byte 3) so a stale reply for a different object on the same COB-ID is
        // skipped.
        let [idx_lo, idx_hi] = index.to_le_bytes();
        let resp = self.initiate_retry(
            encode_upload_request(index, subindex),
            Some((idx_lo, idx_hi, subindex)),
            |r| r[0] & 0xE0 == 0x40 && r[1] == idx_lo && r[2] == idx_hi && r[3] == subindex,
        )?;
        let cs = resp[0];

        // Expedited: e=1 (bit 1 set)
        if cs & 0xE0 == 0x40 && cs & 0x02 != 0 {
            let n = ((cs >> 2) & 0x03) as usize; // bytes not used
            let data_len = 4usize.saturating_sub(n);
            let bytes = &resp[4..4 + data_len];
            return Ok(String::from_utf8_lossy(bytes)
                .trim_end_matches('\0')
                .to_string());
        }

        // Segmented: e=0, s may be 0 or 1
        if decode_segmented_upload_initiate(&resp).is_none() {
            return Err(SdoError::Protocol(format!(
                "expected upload initiate response, got 0x{cs:02X}"
            )));
        }

        let mut buf: Vec<u8> = Vec::new();
        let mut toggle = false;
        loop {
            self.send(encode_upload_segment_ack(toggle))?;
            let seg = self.recv_response()?;
            let (payload, is_last) = decode_upload_segment_response(&seg).ok_or_else(|| {
                SdoError::Protocol(format!(
                    "expected upload segment response, got 0x{:02X}",
                    seg[0]
                ))
            })?;
            buf.extend_from_slice(&payload);
            toggle = !toggle;
            if is_last {
                break;
            }
        }

        Ok(String::from_utf8_lossy(&buf)
            .trim_end_matches('\0')
            .to_string())
    }

    /// Send an SDO abort frame to the node, resetting its SDO server state.
    ///
    /// Call this at startup to clear any in-progress transfer left by a
    /// previously interrupted session. No response is expected.
    pub fn send_abort(&mut self, index: u16, subindex: u8) -> Result<(), SdoError> {
        // Abort code 0x0504_0000 = "SDO protocol timed out"
        self.send(encode_abort(index, subindex, 0x0504_0000))
    }

    /// Write an 8-bit value to the node via expedited SDO download.
    ///
    /// Use this for objects whose CANopen data type is UNSIGNED8 (e.g. 0x1F51
    /// Program Control) to avoid SDO abort 0x06070012 "length too high".
    pub fn write_u8(&mut self, index: u16, subindex: u8, value: u8) -> Result<(), SdoError> {
        self.drain_rx();
        let frame = encode_download_expedited(index, subindex, &[value])
            .ok_or_else(|| SdoError::Protocol("expedited download data > 4 bytes".into()))?;
        self.send(frame)?;
        // Match the echoed multiplexer (index in bytes 1-2, subindex in byte 3)
        // as well as the 0x60 command specifier so a stale download-ack for a
        // different object on the same COB-ID is not mis-associated.
        let [idx_lo, idx_hi] = index.to_le_bytes();
        let resp = self.recv_response_matching(Some((idx_lo, idx_hi, subindex)), |r| {
            is_download_initiate_ack(r) && r[1] == idx_lo && r[2] == idx_hi && r[3] == subindex
        })?;
        if !is_download_initiate_ack(&resp) {
            return Err(SdoError::Protocol(format!(
                "expected download ack (0x60), got 0x{:02X}",
                resp[0]
            )));
        }
        Ok(())
    }

    /// Download a large byte buffer via segmented SDO transfer to the node.
    pub fn download_segmented(
        &mut self,
        index: u16,
        subindex: u8,
        data: &[u8],
    ) -> Result<(), SdoError> {
        // Initiate. Match the echoed multiplexer as well as the 0x60 command
        // specifier so a stale ack for a different object on the same COB-ID is
        // skipped.
        let [idx_lo, idx_hi] = index.to_le_bytes();
        let resp = self.initiate_retry(
            encode_download_initiate_segmented(index, subindex, data.len() as u32),
            Some((idx_lo, idx_hi, subindex)),
            |r| is_download_initiate_ack(r) && r[1] == idx_lo && r[2] == idx_hi && r[3] == subindex,
        )?;
        if !is_download_initiate_ack(&resp) {
            return Err(SdoError::Protocol(format!(
                "segmented initiate ack expected (0x60), got 0x{:02X}",
                resp[0]
            )));
        }

        // Send segments
        let mut toggle = false;
        let mut offset = 0;
        while offset < data.len() {
            let end = (offset + 7).min(data.len());
            let chunk = &data[offset..end];
            let is_last = end == data.len();
            self.send(encode_download_segment(chunk, toggle, is_last))?;
            let resp = self.recv_response_matching(None, |r| is_download_segment_ack(r, toggle))?;
            if !is_download_segment_ack(&resp, toggle) {
                return Err(SdoError::Protocol(format!(
                    "segment ack mismatch at offset {offset}: got 0x{:02X}",
                    resp[0]
                )));
            }
            toggle = !toggle;
            offset = end;
        }
        Ok(())
    }

    /// Download `data` via segmented SDO transfer, streaming every segment
    /// without reading its per-segment ack.
    ///
    /// For adapters that drop SDO responses (Apex, kodezine/RustyCAN#107): a
    /// dropped ack cannot stall the transfer because no ack is read. Correctness
    /// is confirmed by the caller's flash-status poll, not by the acks. Sends
    /// back-pressure on a full adapter TX queue so the stream rate matches the
    /// adapter's drain rate and the device TX FIFO does not overrun. The final
    /// ack (if any) is drained best-effort.
    pub fn download_segmented_no_wait(
        &mut self,
        index: u16,
        subindex: u8,
        data: &[u8],
    ) -> Result<(), SdoError> {
        // Initiate — this one exchange must be acked so the server enters the
        // download state before segments stream in.
        let [idx_lo, idx_hi] = index.to_le_bytes();
        let resp = self.initiate_retry(
            encode_download_initiate_segmented(index, subindex, data.len() as u32),
            Some((idx_lo, idx_hi, subindex)),
            |r| is_download_initiate_ack(r) && r[1] == idx_lo && r[2] == idx_hi && r[3] == subindex,
        )?;
        if !is_download_initiate_ack(&resp) {
            return Err(SdoError::Protocol(format!(
                "segmented initiate ack expected (0x60), got 0x{:02X}",
                resp[0]
            )));
        }

        let mut toggle = false;
        let mut offset = 0;
        while offset < data.len() {
            let end = (offset + 7).min(data.len());
            let chunk = &data[offset..end];
            let is_last = end == data.len();
            self.send_backpressured(encode_download_segment(chunk, toggle, is_last))?;
            toggle = !toggle;
            offset = end;
        }
        // The server's final ack may be dropped by the adapter, so a timeout
        // here is fine — the caller's flash-status poll is authoritative. But an
        // explicit abort is a real failure and must not be swallowed.
        match self.recv_response_matching_within(Duration::from_millis(300), None, |_| true) {
            Err(e @ SdoError::Abort(_)) => Err(e),
            _ => Ok(()),
        }
    }

    /// Send a frame, waiting and retrying while the adapter TX queue is full so
    /// the caller streams at the adapter's drain rate instead of erroring.
    fn send_backpressured(&mut self, data: [u8; 8]) -> Result<(), SdoError> {
        let frame = self.make_request_frame(data);
        let deadline = Instant::now() + self.cfg.timeout;
        loop {
            match self.adapter.send(&frame) {
                Ok(()) => return Ok(()),
                // Only back off on a momentarily full TX queue; real send
                // failures (interface down, device unplugged) surface at once.
                Err(AdapterError::TxQueueFull) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => return Err(SdoError::Adapter(e)),
            }
        }
    }

    /// Download a large byte buffer via block SDO transfer to the node.
    pub fn download_block(
        &mut self,
        index: u16,
        subindex: u8,
        data: &[u8],
    ) -> Result<(), SdoError> {
        // ── Initiate ─────────────────────────────────────────────────────────
        // The block-initiate response echoes the multiplexer (index in bytes
        // 1-2, subindex in byte 3); match it alongside the CS so a stale
        // initiate response for a different object on the same COB-ID is
        // skipped. (The later sub-block/end responses carry only ackseq/blksize
        // /CRC, so there is no multiplexer to match on those.)
        let [idx_lo, idx_hi] = index.to_le_bytes();
        let resp = self.initiate_retry(
            encode_block_download_initiate(index, subindex, data.len() as u32, true),
            Some((idx_lo, idx_hi, subindex)),
            |r| {
                decode_block_download_initiate_response(r).is_some()
                    && r[1] == idx_lo
                    && r[2] == idx_hi
                    && r[3] == subindex
            },
        )?;
        let (mut blksize, crc_supported) = decode_block_download_initiate_response(&resp)
            .ok_or_else(|| {
                SdoError::Protocol(format!(
                    "expected block initiate response (0xA0/0xA4), got 0x{:02X}",
                    resp[0]
                ))
            })?;
        if blksize == 0 {
            blksize = DEFAULT_BLOCK_SIZE;
        }
        // Per CiA 301, blksize is 1-127 segments. Clamp defensively so a
        // misbehaving server cannot push us to emit invalid sequence numbers.
        blksize = blksize.min(127);

        // ── Sub-blocks ───────────────────────────────────────────────────────
        // Send the payload as a sequence of sub-blocks of up to `blksize`
        // segments (7 bytes each). After each sub-block the server acknowledges
        // the sequence number of the last segment it received correctly; if that
        // is fewer than we sent, the missing segments are retransmitted starting
        // from the first un-acknowledged one.
        let mut offset = 0;
        let mut stall_count: u32 = 0;

        while offset < data.len() {
            let block_start = offset;
            let block_end_data = (block_start + blksize as usize * 7).min(data.len());

            // Send up to `blksize` segments for this sub-block.
            let mut seqno: u8 = 1;
            let mut seg_offset = block_start;
            while seg_offset < block_end_data {
                let seg_end = (seg_offset + 7).min(data.len());
                let chunk = &data[seg_offset..seg_end];
                let is_last_seg = seg_end == data.len();

                // seqno is 1-127; the very last segment of the transfer sets bit 7.
                let cs_seqno = if is_last_seg { seqno | 0x80 } else { seqno };
                let mut frame_data = [0u8; 8];
                frame_data[0] = cs_seqno;
                for (i, &b) in chunk.iter().enumerate().take(7) {
                    frame_data[1 + i] = b;
                }
                self.send(frame_data)?;

                seqno += 1;
                seg_offset = seg_end;
            }
            let segs_sent = seqno - 1;

            // Wait for sub-block acknowledgement.
            let resp = self.recv_response_matching(None, |r| {
                decode_block_download_subblock_response(r).is_some()
            })?;
            let (ackseq, new_blksize) =
                decode_block_download_subblock_response(&resp).ok_or_else(|| {
                    SdoError::Protocol(format!(
                        "expected block sub-block response (0xA2), got 0x{:02X}",
                        resp[0]
                    ))
                })?;

            if ackseq > segs_sent {
                return Err(SdoError::Protocol(format!(
                    "block ack out of range: sent {segs_sent} segments, server acked {ackseq}"
                )));
            }

            // Advance past the segments the server confirmed. When `ackseq` is
            // less than we sent, the remaining segments were lost on the wire and
            // are retransmitted on the next iteration from this offset.
            //
            // Per CiA 301 every sub-block is numbered starting at seqno 1, so
            // "retransmission" means continuing from the acknowledged byte offset
            // in a fresh sub-block; sequence numbers are intentionally not
            // preserved across sub-blocks.
            //
            // Clamp to `data.len()`: when the final segment is shorter than 7
            // bytes, `ackseq * 7` can point past the end of the payload, so keep
            // `offset` within range to make the loop invariant explicit.
            offset = (block_start + ackseq as usize * 7).min(data.len());

            // Guard against a livelock where the server keeps acknowledging zero
            // segments (nothing is getting through).
            if ackseq == 0 {
                stall_count += 1;
                if stall_count > 16 {
                    return Err(SdoError::Protocol(
                        "block download stalled: server repeatedly acknowledged 0 segments".into(),
                    ));
                }
            } else {
                stall_count = 0;
            }

            blksize = if new_blksize > 0 {
                new_blksize.min(127)
            } else {
                DEFAULT_BLOCK_SIZE
            };
        }

        // ── End ──────────────────────────────────────────────────────────────
        // n = number of bytes in last segment that do not contain data
        let last_seg_data = data.len() % 7;
        let n = if last_seg_data == 0 {
            0
        } else {
            (7 - last_seg_data) as u8
        };
        // Only send a real CRC when the server negotiated CRC support; otherwise
        // the CRC field is ignored, so send 0.
        let crc = if crc_supported {
            calculate_crc16(data)
        } else {
            0
        };
        self.send(encode_block_download_end(n, crc))?;

        let resp = self.recv_response_matching(None, |r| decode_block_download_end_response(r))?;
        if !decode_block_download_end_response(&resp) {
            return Err(SdoError::Protocol(format!(
                "expected block end ack (0xA1), got 0x{:02X}",
                resp[0]
            )));
        }
        Ok(())
    }

    /// Download `data` to the node using the specified [`SdocType`].
    ///
    /// For data ≤ 4 bytes, uses an expedited transfer regardless of `mode`.
    pub fn download(
        &mut self,
        index: u16,
        subindex: u8,
        data: &[u8],
        mode: SdocType,
    ) -> Result<(), SdoError> {
        if data.len() <= 4 {
            self.drain_rx();
            let frame = encode_download_expedited(index, subindex, data)
                .ok_or_else(|| SdoError::Protocol("expedited data > 4 bytes".into()))?;
            self.send(frame)?;
            // Match the echoed multiplexer as well as the 0x60 command specifier
            // so a stale ack for a different object on the same COB-ID is skipped.
            let [idx_lo, idx_hi] = index.to_le_bytes();
            let resp = self.recv_response_matching(Some((idx_lo, idx_hi, subindex)), |r| {
                is_download_initiate_ack(r) && r[1] == idx_lo && r[2] == idx_hi && r[3] == subindex
            })?;
            if !is_download_initiate_ack(&resp) {
                return Err(SdoError::Protocol(format!(
                    "expected download ack (0x60), got 0x{:02X}",
                    resp[0]
                )));
            }
            return Ok(());
        }
        match mode {
            SdocType::Segmented => self.download_segmented(index, subindex, data),
            SdocType::Block => self.download_block(index, subindex, data),
            SdocType::SegmentedNoWait => self.download_segmented_no_wait(index, subindex, data),
        }
    }
}
