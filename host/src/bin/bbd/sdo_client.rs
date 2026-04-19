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
    decode_block_download_subblock_response, encode_block_download_end,
    encode_block_download_initiate, encode_block_download_subblock, encode_download_expedited,
    encode_download_initiate_segmented, encode_download_segment, encode_upload_request,
    is_download_initiate_ack, is_download_segment_ack,
};

// ─── Constants ───────────────────────────────────────────────────────────────

/// Default number of segments per block for block-mode SDO downloads.
const DEFAULT_BLOCK_SIZE: u8 = 16;

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
}

impl SdocType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Segmented),
            2 => Some(Self::Block),
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

    /// Wait for a CAN frame from the node's SDO response COB-ID.
    ///
    /// Frames from other COB-IDs are silently discarded. Returns the 8-byte
    /// data payload of the matching frame, or [`SdoError::Timeout`].
    fn recv_response(&mut self) -> Result<[u8; 8], SdoError> {
        let expected_cob = self.response_cob_id();
        let deadline = Instant::now() + self.cfg.timeout;

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
                    // Check for SDO abort (CS = 0x80)
                    if raw[0] == 0x80 {
                        let code = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
                        return Err(SdoError::Abort(code));
                    }
                    let mut out = [0u8; 8];
                    out.copy_from_slice(&raw[..8]);
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

    // ── Public SDO operations ─────────────────────────────────────────────────

    /// Read a 32-bit value from the node via expedited SDO upload.
    pub fn read_u32(&mut self, index: u16, subindex: u8) -> Result<u32, SdoError> {
        self.send(encode_upload_request(index, subindex))?;
        let resp = self.recv_response()?;
        // Validate: scs=2 (bits 7-5 = 010), e=1 (bit 1), s=1 (bit 0) → 0x4B / 0x4F
        let cs = resp[0];
        if cs & 0xE0 != 0x40 || cs & 0x02 == 0 {
            return Err(SdoError::Protocol(format!(
                "expected expedited upload response (cs=0x4x), got 0x{cs:02X}"
            )));
        }
        Ok(u32::from_le_bytes([resp[4], resp[5], resp[6], resp[7]]))
    }

    /// Write a 32-bit value to the node via expedited SDO download.
    pub fn write_u32(&mut self, index: u16, subindex: u8, value: u32) -> Result<(), SdoError> {
        let data = value.to_le_bytes();
        let frame = encode_download_expedited(index, subindex, &data)
            .ok_or_else(|| SdoError::Protocol("expedited download data > 4 bytes".into()))?;
        self.send(frame)?;
        let resp = self.recv_response()?;
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
        // Initiate
        self.send(encode_download_initiate_segmented(
            index,
            subindex,
            data.len() as u32,
        ))?;
        let resp = self.recv_response()?;
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
            let resp = self.recv_response()?;
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

    /// Download a large byte buffer via block SDO transfer to the node.
    pub fn download_block(
        &mut self,
        index: u16,
        subindex: u8,
        data: &[u8],
    ) -> Result<(), SdoError> {
        // ── Initiate ─────────────────────────────────────────────────────────
        self.send(encode_block_download_initiate(
            index,
            subindex,
            data.len() as u32,
            true, // CRC enabled
        ))?;
        let resp = self.recv_response()?;
        let mut blksize = decode_block_download_initiate_response(&resp).ok_or_else(|| {
            SdoError::Protocol(format!(
                "expected block initiate response (0xA4), got 0x{:02X}",
                resp[0]
            ))
        })?;
        if blksize == 0 {
            blksize = DEFAULT_BLOCK_SIZE;
        }

        // ── Sub-blocks ───────────────────────────────────────────────────────
        let mut offset = 0;
        let mut seqno: u8 = 1;

        while offset < data.len() {
            // Send up to `blksize` segments of 7 bytes each
            let block_start = offset;
            let block_end_data = (block_start + blksize as usize * 7).min(data.len());
            let is_last_block = block_end_data == data.len();

            while offset < block_end_data {
                let seg_end = (offset + 7).min(data.len());
                let chunk = &data[offset..seg_end];
                let is_last_seg = seg_end == data.len();

                // For block download sub-blocks, seqno is 1-127.
                // The last segment of the last sub-block has bit 7 set.
                let cs_seqno = if is_last_seg { seqno | 0x80 } else { seqno };
                let mut frame_data = [0u8; 8];
                frame_data[0] = cs_seqno;
                for (i, &b) in chunk.iter().enumerate().take(7) {
                    frame_data[1 + i] = b;
                }
                // Use the helper to build the frame (ignores is_last — we set the bit manually above)
                let _ = encode_block_download_subblock(seqno, chunk, false);
                self.send(frame_data)?;

                seqno += 1;
                offset = seg_end;
            }

            // Wait for sub-block acknowledgement
            let resp = self.recv_response()?;
            let (ackseq, new_blksize) =
                decode_block_download_subblock_response(&resp).ok_or_else(|| {
                    SdoError::Protocol(format!(
                        "expected block sub-block response (0xA2), got 0x{:02X}",
                        resp[0]
                    ))
                })?;

            // If ackseq < seqno-1 some segments were lost — retransmit from ackseq+1.
            // For simplicity (and matching the C implementation) we treat any ack < seqno-1
            // as a protocol error. A more robust implementation would retransmit.
            let expected_ack = seqno - 1;
            if ackseq != expected_ack {
                return Err(SdoError::Protocol(format!(
                    "block ack mismatch: expected {expected_ack}, got {ackseq}"
                )));
            }

            blksize = if new_blksize > 0 {
                new_blksize
            } else {
                DEFAULT_BLOCK_SIZE
            };
            seqno = 1;

            if is_last_block {
                break;
            }
        }

        // ── End ──────────────────────────────────────────────────────────────
        // n = number of bytes in last segment that do not contain data
        let last_seg_data = data.len() % 7;
        let n = if last_seg_data == 0 {
            0
        } else {
            (7 - last_seg_data) as u8
        };
        let crc = calculate_crc16(data);
        self.send(encode_block_download_end(n, crc))?;

        let resp = self.recv_response()?;
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
            let frame = encode_download_expedited(index, subindex, data)
                .ok_or_else(|| SdoError::Protocol("expedited data > 4 bytes".into()))?;
            self.send(frame)?;
            let resp = self.recv_response()?;
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
        }
    }
}
