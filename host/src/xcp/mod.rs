//! XCP (Universal Measurement and Calibration Protocol) on CAN — transport layer.
//!
//! XCP on CAN uses two configured CAN identifiers:
//!
//! * **CRO** (Command Receive Object) — master → slave command frames.
//! * **DTO** (Data Transmit Object) — slave → master frames, carrying command
//!   responses, error/event/service packets, and DAQ measurement data.
//!
//! Because the default XCP CRO/DTO identifiers overlap the CANopen SDO range
//! (0x600+), XCP frames are recognised **only** on the explicitly configured
//! [`XcpConfig::cro_id`] / [`XcpConfig::dto_id`]. The session layer therefore
//! consults [`classify`] before falling back to the CANopen classifier.
//!
//! Multi-byte fields (addresses, DAQ pointers) are transmitted in the slave's
//! byte order, which is negotiated in the CONNECT response and tracked as
//! [`ByteOrder`].

pub mod a2l;
pub mod command;
pub mod daq;

use host_can::frame::CanFrame;

/// Byte order used by the slave for multi-byte protocol fields.
///
/// Negotiated from the `COMM_MODE_BASIC` byte of the CONNECT response. Defaults
/// to little-endian, the most common configuration, until a CONNECT response is
/// observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ByteOrder {
    /// Intel / little-endian (LSB first). `COMM_MODE_BASIC` bit 0 = 0.
    #[default]
    LittleEndian,
    /// Motorola / big-endian (MSB first). `COMM_MODE_BASIC` bit 0 = 1.
    BigEndian,
}

impl ByteOrder {
    /// Encode a `u32` address/value into 4 bytes in this byte order.
    pub fn u32_to_bytes(self, value: u32) -> [u8; 4] {
        match self {
            ByteOrder::LittleEndian => value.to_le_bytes(),
            ByteOrder::BigEndian => value.to_be_bytes(),
        }
    }

    /// Decode the first 4 bytes of `bytes` as a `u32` in this byte order.
    ///
    /// Missing bytes are treated as zero so short frames decode defensively.
    pub fn u32_from_bytes(self, bytes: &[u8]) -> u32 {
        let b = |i: usize| bytes.get(i).copied().unwrap_or(0);
        let raw = [b(0), b(1), b(2), b(3)];
        match self {
            ByteOrder::LittleEndian => u32::from_le_bytes(raw),
            ByteOrder::BigEndian => u32::from_be_bytes(raw),
        }
    }

    /// Decode the first 2 bytes of `bytes` as a `u16` in this byte order.
    pub fn u16_from_bytes(self, bytes: &[u8]) -> u16 {
        let b = |i: usize| bytes.get(i).copied().unwrap_or(0);
        let raw = [b(0), b(1)];
        match self {
            ByteOrder::LittleEndian => u16::from_le_bytes(raw),
            ByteOrder::BigEndian => u16::from_be_bytes(raw),
        }
    }

    /// Encode a `u16` into 2 bytes in this byte order.
    pub fn u16_to_bytes(self, value: u16) -> [u8; 2] {
        match self {
            ByteOrder::LittleEndian => value.to_le_bytes(),
            ByteOrder::BigEndian => value.to_be_bytes(),
        }
    }
}

/// XCP-on-CAN identifier configuration.
///
/// Both identifiers are full CAN IDs (11-bit standard or 29-bit extended) so
/// extended-ID XCP deployments are supported without aliasing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XcpConfig {
    /// CRO identifier — master → slave command frames.
    pub cro_id: u32,
    /// DTO identifier — slave → master response / event / DAQ frames.
    pub dto_id: u32,
}

/// Classification of an XCP frame relative to the configured CRO/DTO IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XcpFrameType {
    /// Command frame (master → slave, matches CRO ID). Byte 0 is the command code.
    Command,
    /// Positive command response (`RES`, DTO byte 0 = 0xFF).
    Response,
    /// Error / negative response (`ERR`, DTO byte 0 = 0xFE).
    Error,
    /// Asynchronous event packet (`EV`, DTO byte 0 = 0xFD).
    Event,
    /// Service request packet (`SERV`, DTO byte 0 = 0xFC).
    ServiceRequest,
    /// DAQ measurement data. Carries the packet identifier (PID ≤ 0xFB) that
    /// selects the originating DAQ list / ODT.
    Daq(u8),
}

/// Packet-identifier byte for a positive command response (`RES`).
pub const PID_RES: u8 = 0xFF;
/// Packet-identifier byte for an error / negative response (`ERR`).
pub const PID_ERR: u8 = 0xFE;
/// Packet-identifier byte for an asynchronous event packet (`EV`).
pub const PID_EV: u8 = 0xFD;
/// Packet-identifier byte for a service request packet (`SERV`).
pub const PID_SERV: u8 = 0xFC;

/// Classify a CAN frame (by identifier and payload) against the XCP config.
///
/// Returns `None` when `can_id` matches neither the CRO nor the DTO identifier,
/// signalling that the frame is not XCP traffic and should fall through to the
/// CANopen / DBC classifiers.
pub fn classify(config: &XcpConfig, can_id: u32, data: &[u8]) -> Option<XcpFrameType> {
    if can_id == config.cro_id {
        return Some(XcpFrameType::Command);
    }
    if can_id == config.dto_id {
        let pid = data.first().copied().unwrap_or(0);
        return Some(match pid {
            PID_RES => XcpFrameType::Response,
            PID_ERR => XcpFrameType::Error,
            PID_EV => XcpFrameType::Event,
            PID_SERV => XcpFrameType::ServiceRequest,
            other => XcpFrameType::Daq(other),
        });
    }
    None
}

/// Extract the full CAN identifier (11-bit standard or 29-bit extended) from a
/// frame, matching the keying used by [`classify`].
pub fn full_can_id(frame: &CanFrame) -> u32 {
    use embedded_can::{Frame, Id};
    match frame.id() {
        Id::Standard(sid) => sid.as_raw() as u32,
        Id::Extended(eid) => eid.as_raw() & 0x1FFF_FFFF,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CFG: XcpConfig = XcpConfig {
        cro_id: 0x600,
        dto_id: 0x601,
    };

    #[test]
    fn cro_is_command() {
        assert_eq!(
            classify(&CFG, 0x600, &[0xFF, 0x00]),
            Some(XcpFrameType::Command)
        );
    }

    #[test]
    fn dto_positive_response() {
        assert_eq!(
            classify(&CFG, 0x601, &[0xFF, 0x01]),
            Some(XcpFrameType::Response)
        );
    }

    #[test]
    fn dto_error() {
        assert_eq!(
            classify(&CFG, 0x601, &[0xFE, 0x20]),
            Some(XcpFrameType::Error)
        );
    }

    #[test]
    fn dto_event_and_serv() {
        assert_eq!(classify(&CFG, 0x601, &[0xFD]), Some(XcpFrameType::Event));
        assert_eq!(
            classify(&CFG, 0x601, &[0xFC]),
            Some(XcpFrameType::ServiceRequest)
        );
    }

    #[test]
    fn dto_daq_pid() {
        assert_eq!(classify(&CFG, 0x601, &[0x00]), Some(XcpFrameType::Daq(0)));
        assert_eq!(
            classify(&CFG, 0x601, &[0x1F]),
            Some(XcpFrameType::Daq(0x1F))
        );
    }

    #[test]
    fn unrelated_id_is_none() {
        assert_eq!(classify(&CFG, 0x181, &[0x00]), None);
        assert_eq!(classify(&CFG, 0x000, &[0x00]), None);
    }

    #[test]
    fn empty_dto_defaults_to_daq_zero() {
        assert_eq!(classify(&CFG, 0x601, &[]), Some(XcpFrameType::Daq(0)));
    }

    #[test]
    fn byte_order_roundtrip() {
        assert_eq!(
            ByteOrder::LittleEndian.u32_to_bytes(0x1234_5678),
            [0x78, 0x56, 0x34, 0x12]
        );
        assert_eq!(
            ByteOrder::BigEndian.u32_to_bytes(0x1234_5678),
            [0x12, 0x34, 0x56, 0x78]
        );
        assert_eq!(
            ByteOrder::LittleEndian.u32_from_bytes(&[0x78, 0x56, 0x34, 0x12]),
            0x1234_5678
        );
        assert_eq!(
            ByteOrder::BigEndian.u32_from_bytes(&[0x12, 0x34, 0x56, 0x78]),
            0x1234_5678
        );
    }

    #[test]
    fn u32_from_short_slice_is_defensive() {
        assert_eq!(
            ByteOrder::LittleEndian.u32_from_bytes(&[0x01, 0x02]),
            0x0000_0201
        );
    }
}
