//! KCAN wire frame layout.
//!
//! Every USB bulk transfer in either direction carries exactly one
//! [`KCanFrame`] of [`KCAN_FRAME_SIZE`] bytes, little-endian.
//!
//! # Layout
//!
//! | Offset | Size | Field           | Description                                       |
//! |--------|------|-----------------|---------------------------------------------------|
//! | 0      | 1    | `magic`         | `0xCA` — framing sanity marker                    |
//! | 1      | 1    | `version`       | `0x01`                                            |
//! | 2      | 1    | `frame_type`    | [`FrameType`]                                     |
//! | 3      | 1    | `flags`         | [`FrameFlags`] bitfield                           |
//! | 4      | 4    | `can_id`        | CAN identifier (11-bit or 29-bit, LE)             |
//! | 8      | 4    | `timestamp_us`  | µs since bus-on snapshotted in FDCAN ISR (LE)     |
//! | 12     | 1    | `dlc`           | Data length code 0–8 (classic) or 0–15 (FD)      |
//! | 13     | 1    | `channel`       | Always 0 for single-channel dongles               |
//! | 14     | 2    | `seq`           | 16-bit monotonic counter (replay detection)       |
//! | 16     | 64   | `data`          | Payload (8 active bytes classic, 64 max for FD)   |
//!
//! Total: **80 bytes**.

/// First byte of every KCAN frame — framing sanity check.
pub const KCAN_MAGIC: u8 = 0xCA;

/// Current protocol version.
pub const KCAN_VERSION: u8 = 0x01;

/// Size of one KCAN frame in bytes.
pub const KCAN_FRAME_SIZE: usize = 80;

/// Maximum data payload bytes (reserves space for CAN FD).
pub const KCAN_MAX_DATA: usize = 64;

// ─── Frame type ───────────────────────────────────────────────────────────────

/// Indicates what data a frame carries.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrameType {
    /// A CAN frame received from the bus (device→host).
    Data = 0x01,
    /// Echo of a frame that was just transmitted (device→host).
    ///
    /// `timestamp_us` reflects the exact moment the last bit left the bus.
    TxEcho = 0x02,
    /// A CAN bus error frame was observed (device→host).
    BusError = 0x03,
    /// Periodic dongle status (device→host): error counters, bus state.
    Status = 0x04,
}

impl FrameType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x01 => Some(Self::Data),
            0x02 => Some(Self::TxEcho),
            0x03 => Some(Self::BusError),
            0x04 => Some(Self::Status),
            _ => None,
        }
    }
}

// ─── Frame flags ──────────────────────────────────────────────────────────────

/// Bitfield flags in byte 3 of the frame header.
pub struct FrameFlags;

impl FrameFlags {
    /// Extended Frame Format (29-bit CAN ID).
    pub const EFF: u8 = 1 << 0;
    /// Remote Transmission Request.
    pub const RTR: u8 = 1 << 1;
    /// CAN FD frame.
    pub const FD: u8 = 1 << 2;
    /// CAN FD Bit Rate Switch.
    pub const BRS: u8 = 1 << 3;
    /// CAN FD Error State Indicator.
    pub const ESI: u8 = 1 << 4;
}

// ─── Frame struct ─────────────────────────────────────────────────────────────

/// An 80-byte KCAN wire frame.
///
/// All multi-byte fields are **little-endian**.
///
/// # Constructing
///
/// Use [`KCanFrame::new_data`] for RX frames, [`KCanFrame::new_tx`] for
/// frames the host wants the dongle to transmit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KCanFrame {
    pub magic: u8,
    pub version: u8,
    pub frame_type: u8,
    pub flags: u8,
    /// CAN identifier (standard 11-bit or extended 29-bit).
    pub can_id: u32,
    /// µs since bus-on, captured in the FDCAN RX ISR from TIM2.
    ///
    /// Set to 0 for TX frames sent from the host (dongle ignores it).
    pub timestamp_us: u32,
    pub dlc: u8,
    pub channel: u8,
    /// 16-bit monotonic counter; increments for every frame in this direction.
    pub seq: u16,
    /// Payload bytes.  Only `dlc` bytes are meaningful for classic CAN.
    /// Padded with zeros to 64 bytes.
    pub data: [u8; KCAN_MAX_DATA],
}

impl KCanFrame {
    /// Create an RX frame (device→host).
    pub fn new_data(
        can_id: u32,
        flags: u8,
        dlc: u8,
        data: &[u8],
        timestamp_us: u32,
        seq: u16,
    ) -> Self {
        let mut d = [0u8; KCAN_MAX_DATA];
        let len = (dlc as usize).min(data.len()).min(KCAN_MAX_DATA);
        d[..len].copy_from_slice(&data[..len]);
        Self {
            magic: KCAN_MAGIC,
            version: KCAN_VERSION,
            frame_type: FrameType::Data as u8,
            flags,
            can_id,
            timestamp_us,
            dlc,
            channel: 0,
            seq,
            data: d,
        }
    }

    /// Create a TX frame (host→device).
    pub fn new_tx(can_id: u32, flags: u8, dlc: u8, data: &[u8], seq: u16) -> Self {
        Self::new_data(can_id, flags, dlc, data, 0, seq)
    }

    /// Create a TX echo frame (device→host after successful transmission).
    pub fn new_tx_echo(
        can_id: u32,
        flags: u8,
        dlc: u8,
        data: &[u8],
        timestamp_us: u32,
        seq: u16,
    ) -> Self {
        let mut f = Self::new_data(can_id, flags, dlc, data, timestamp_us, seq);
        f.frame_type = FrameType::TxEcho as u8;
        f
    }

    /// Serialize to the 80-byte on-wire representation.
    pub fn to_bytes(&self) -> [u8; KCAN_FRAME_SIZE] {
        let mut out = [0u8; KCAN_FRAME_SIZE];
        out[0] = self.magic;
        out[1] = self.version;
        out[2] = self.frame_type;
        out[3] = self.flags;
        out[4..8].copy_from_slice(&self.can_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.timestamp_us.to_le_bytes());
        out[12] = self.dlc;
        out[13] = self.channel;
        out[14..16].copy_from_slice(&self.seq.to_le_bytes());
        out[16..80].copy_from_slice(&self.data);
        out
    }

    /// Deserialize from the 80-byte on-wire representation.
    ///
    /// Returns `None` if `magic` or `version` do not match.
    pub fn from_bytes(b: &[u8; KCAN_FRAME_SIZE]) -> Option<Self> {
        if b[0] != KCAN_MAGIC || b[1] != KCAN_VERSION {
            return None;
        }
        let mut data = [0u8; KCAN_MAX_DATA];
        data.copy_from_slice(&b[16..80]);
        Some(Self {
            magic: b[0],
            version: b[1],
            frame_type: b[2],
            flags: b[3],
            can_id: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            timestamp_us: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
            dlc: b[12],
            channel: b[13],
            seq: u16::from_le_bytes([b[14], b[15]]),
            data,
        })
    }
}
