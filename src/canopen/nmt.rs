use serde::Serialize;

/// State of a CANopen node as reported in heartbeat / bootup messages.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum NmtState {
    Bootup,
    Stopped,
    Operational,
    PreOperational,
    Unknown(u8),
}

impl std::fmt::Display for NmtState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NmtState::Bootup => write!(f, "BOOTUP"),
            NmtState::Stopped => write!(f, "STOPPED"),
            NmtState::Operational => write!(f, "OPERATIONAL"),
            NmtState::PreOperational => write!(f, "PRE-OPERATIONAL"),
            NmtState::Unknown(b) => write!(f, "UNKNOWN(0x{b:02X})"),
        }
    }
}

/// NMT master command code sent in a COB-ID 0x000 frame.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum NmtCommand {
    StartRemoteNode,
    StopRemoteNode,
    EnterPreOperational,
    ResetNode,
    ResetCommunication,
    Unknown(u8),
}

impl std::fmt::Display for NmtCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NmtCommand::StartRemoteNode => write!(f, "START"),
            NmtCommand::StopRemoteNode => write!(f, "STOP"),
            NmtCommand::EnterPreOperational => write!(f, "ENTER_PRE_OP"),
            NmtCommand::ResetNode => write!(f, "RESET_NODE"),
            NmtCommand::ResetCommunication => write!(f, "RESET_COMM"),
            NmtCommand::Unknown(b) => write!(f, "UNKNOWN(0x{b:02X})"),
        }
    }
}

/// Decoded NMT-layer event.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NmtEvent {
    /// Master-issued command (COB-ID 0x000).
    Command {
        command: NmtCommand,
        /// Target node-id; 0 means all nodes.
        target_node: u8,
    },
    /// Heartbeat / bootup from a node (COB-ID 0x700 + node_id).
    Heartbeat { node_id: u8, state: NmtState },
}

/// Decode an NMT master command frame (COB-ID 0x000).
pub fn decode_nmt_command(data: &[u8]) -> Option<NmtEvent> {
    if data.len() < 2 {
        return None;
    }
    let command = match data[0] {
        0x01 => NmtCommand::StartRemoteNode,
        0x02 => NmtCommand::StopRemoteNode,
        0x80 => NmtCommand::EnterPreOperational,
        0x81 => NmtCommand::ResetNode,
        0x82 => NmtCommand::ResetCommunication,
        other => NmtCommand::Unknown(other),
    };
    Some(NmtEvent::Command {
        command,
        target_node: data[1],
    })
}

/// Decode a heartbeat / bootup frame (COB-ID 0x700 + node_id).
pub fn decode_heartbeat(node_id: u8, data: &[u8]) -> Option<NmtEvent> {
    if data.is_empty() {
        return None;
    }
    let state = match data[0] {
        0x00 => NmtState::Bootup,
        0x04 => NmtState::Stopped,
        0x05 => NmtState::Operational,
        0x7F => NmtState::PreOperational,
        other => NmtState::Unknown(other),
    };
    Some(NmtEvent::Heartbeat { node_id, state })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_start_all() {
        let ev = decode_nmt_command(&[0x01, 0x00]).unwrap();
        match ev {
            NmtEvent::Command { command, target_node } => {
                assert_eq!(command, NmtCommand::StartRemoteNode);
                assert_eq!(target_node, 0);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn decode_bootup() {
        let ev = decode_heartbeat(2, &[0x00]).unwrap();
        match ev {
            NmtEvent::Heartbeat { node_id, state } => {
                assert_eq!(node_id, 2);
                assert_eq!(state, NmtState::Bootup);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn decode_operational() {
        let ev = decode_heartbeat(1, &[0x05]).unwrap();
        assert!(matches!(ev, NmtEvent::Heartbeat { state: NmtState::Operational, .. }));
    }

    #[test]
    fn decode_pre_op() {
        let ev = decode_heartbeat(3, &[0x7F]).unwrap();
        assert!(matches!(ev, NmtEvent::Heartbeat { state: NmtState::PreOperational, .. }));
    }

    #[test]
    fn returns_none_on_empty() {
        assert!(decode_nmt_command(&[0x01]).is_none()); // only 1 byte
        assert!(decode_heartbeat(1, &[]).is_none());
    }
}
