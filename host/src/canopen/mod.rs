pub mod nmt;
pub mod pdo;
pub mod sdo;

use embedded_can::Id;
use host_can::frame::CanFrame;

/// High-level classification of a CAN frame by its COB-ID.
#[derive(Debug, Clone, PartialEq)]
pub enum FrameType {
    /// NMT master command (COB-ID 0x000).
    NmtCommand,
    /// SYNC message (COB-ID 0x080).
    Sync,
    /// Emergency message (COB-ID 0x081–0x0FF). Contains node-id.
    Emergency(u8),
    /// Transmit PDO (device → master). `(pdo_number 1–4, node_id)`.
    Tpdo(u8, u8),
    /// Receive PDO (master → device). `(pdo_number 1–4, node_id)`.
    Rpdo(u8, u8),
    /// SDO server response (device → master, COB-ID 0x580–0x5FF).
    SdoResponse(u8),
    /// SDO client request (master → device, COB-ID 0x600–0x67F).
    SdoRequest(u8),
    /// NMT heartbeat / bootup message (COB-ID 0x700–0x77F).
    Heartbeat(u8),
    /// COB-ID not mapped to a known CANopen service.
    Unknown(u16),
}

/// Extract the 11-bit COB-ID from a CAN frame.
///
/// CANopen exclusively uses standard (11-bit) IDs. Extended IDs are
/// accepted defensively but only the lower 11 bits are used.
pub fn extract_cob_id(frame: &CanFrame) -> u16 {
    use embedded_can::Frame;
    match frame.id() {
        Id::Standard(sid) => sid.as_raw(),
        Id::Extended(eid) => (eid.as_raw() & 0x7FF) as u16,
    }
}

/// Classify a COB-ID into the corresponding CANopen service.
pub fn classify_frame(cob_id: u16) -> FrameType {
    let node = (cob_id & 0x7F) as u8;
    match cob_id {
        0x000 => FrameType::NmtCommand,
        0x080 => FrameType::Sync,
        0x081..=0x0FF => FrameType::Emergency(node),
        0x180..=0x1FF => FrameType::Tpdo(1, node),
        0x200..=0x27F => FrameType::Rpdo(1, node),
        0x280..=0x2FF => FrameType::Tpdo(2, node),
        0x300..=0x37F => FrameType::Rpdo(2, node),
        0x380..=0x3FF => FrameType::Tpdo(3, node),
        0x400..=0x47F => FrameType::Rpdo(3, node),
        0x480..=0x4FF => FrameType::Tpdo(4, node),
        0x500..=0x57F => FrameType::Rpdo(4, node),
        0x580..=0x5FF => FrameType::SdoResponse(node),
        0x600..=0x67F => FrameType::SdoRequest(node),
        0x700..=0x77F => FrameType::Heartbeat(node),
        other => FrameType::Unknown(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nmt_command() {
        assert_eq!(classify_frame(0x000), FrameType::NmtCommand);
    }

    #[test]
    fn tpdo1_node1() {
        assert_eq!(classify_frame(0x181), FrameType::Tpdo(1, 1));
    }

    #[test]
    fn sdo_response_node3() {
        assert_eq!(classify_frame(0x583), FrameType::SdoResponse(3));
    }

    #[test]
    fn sdo_request_node3() {
        assert_eq!(classify_frame(0x603), FrameType::SdoRequest(3));
    }

    #[test]
    fn heartbeat_node5() {
        assert_eq!(classify_frame(0x705), FrameType::Heartbeat(5));
    }
}
