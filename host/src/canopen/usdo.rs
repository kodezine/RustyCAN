//! CANopen USDO (Universal Service Data Object) — CiA 602.
//!
//! USDO extends SDO to support CAN FD payloads of up to 56 bytes in a single
//! transfer. Default COB-IDs:
//!
//! | Direction           | Default COB-ID |
//! |---------------------|----------------|
//! | Client → Server     | `0x7E5`        |
//! | Server → Client     | `0x7E9`        |
//!
//! # Frame layout (first 8 bytes are header, remainder is data)
//!
//! | Byte | Field              |
//! |------|--------------------|
//! | 0–1  | Node address (LE)  |
//! | 2    | SCS / CSS byte     |
//! | 3    | Command specifier  |
//! | 4–5  | Object index (LE)  |
//! | 6    | Sub-index          |
//! | 7    | Data length (bytes) |
//! | 8+   | Data (up to 56 B)  |

/// Default COB-ID for USDO client-to-server (master → node) frames.
pub const USDO_COB_CLIENT: u16 = 0x7E5;
/// Default COB-ID for USDO server-to-client (node → master) frames.
pub const USDO_COB_SERVER: u16 = 0x7E9;

/// USDO command specifier values (CSS/SCS byte + command byte combined).
pub mod cmd {
    /// Upload initiate (read request from master).
    pub const UPLOAD_INITIATE_REQ: u8 = 0x40;
    /// Upload response (data from node).
    pub const UPLOAD_INITIATE_RSP: u8 = 0x43;
    /// Download initiate (write request from master).
    pub const DOWNLOAD_INITIATE_REQ: u8 = 0x20;
    /// Download response acknowledgement from node.
    pub const DOWNLOAD_INITIATE_RSP: u8 = 0x60;
    /// Abort transfer.
    pub const ABORT: u8 = 0x80;
}

/// A decoded USDO event (request or response).
#[derive(Debug, Clone)]
pub struct UsdoEvent {
    /// Destination / source node ID (from bytes 0–1 of the header).
    pub node_id: u8,
    /// Object index.
    pub index: u16,
    /// Sub-index.
    pub subindex: u8,
    /// Whether this is a read (`true`) or write (`false`) operation.
    pub is_read: bool,
    /// `true` = request (master→node), `false` = response (node→master).
    pub is_request: bool,
    /// Data payload for download requests and upload responses.
    pub data: Vec<u8>,
    /// Abort code when `command == ABORT`, otherwise `None`.
    pub abort_code: Option<u32>,
}

/// Decode a raw USDO frame payload.
///
/// `cob_id` must be either [`USDO_COB_CLIENT`] or [`USDO_COB_SERVER`].
/// Returns `None` when the payload is shorter than 8 bytes (minimum header).
pub fn decode_usdo(cob_id: u16, data: &[u8]) -> Option<UsdoEvent> {
    if data.len() < 8 {
        return None;
    }
    let node_id = data[0]; // Lower byte of the 2-byte node address
    let command = data[3];
    let index = u16::from_le_bytes([data[4], data[5]]);
    let subindex = data[6];
    let data_len = data[7] as usize;
    let payload = data
        .get(8..8 + data_len.min(data.len().saturating_sub(8)))
        .unwrap_or(&[])
        .to_vec();

    let is_request = cob_id == USDO_COB_CLIENT;

    let abort_code = if command == cmd::ABORT && payload.len() >= 4 {
        Some(u32::from_le_bytes([
            payload[0], payload[1], payload[2], payload[3],
        ]))
    } else {
        None
    };

    let is_read = matches!(command, cmd::UPLOAD_INITIATE_REQ | cmd::UPLOAD_INITIATE_RSP);

    Some(UsdoEvent {
        node_id,
        index,
        subindex,
        is_read,
        is_request,
        data: payload,
        abort_code,
    })
}

/// Encode a USDO upload (read) request for the given node, index, sub-index.
///
/// Returns a 64-byte CAN FD payload (USDO header + zero padding).
pub fn encode_usdo_read(node_id: u8, index: u16, subindex: u8) -> Vec<u8> {
    let mut buf = vec![0u8; 64];
    buf[0] = node_id;
    buf[1] = 0x00;
    buf[2] = 0x00;
    buf[3] = cmd::UPLOAD_INITIATE_REQ;
    buf[4] = (index & 0xFF) as u8;
    buf[5] = (index >> 8) as u8;
    buf[6] = subindex;
    buf[7] = 0x00; // no inline data
    buf
}

/// Encode a USDO download (write) request with up to 56 bytes of data.
///
/// Returns a 64-byte CAN FD payload (USDO header + data + zero padding).
pub fn encode_usdo_write(node_id: u8, index: u16, subindex: u8, data: &[u8]) -> Vec<u8> {
    let data_len = data.len().min(56);
    let mut buf = vec![0u8; 64];
    buf[0] = node_id;
    buf[1] = 0x00;
    buf[2] = 0x00;
    buf[3] = cmd::DOWNLOAD_INITIATE_REQ;
    buf[4] = (index & 0xFF) as u8;
    buf[5] = (index >> 8) as u8;
    buf[6] = subindex;
    buf[7] = data_len as u8;
    buf[8..8 + data_len].copy_from_slice(&data[..data_len]);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_read_roundtrip() {
        let payload = encode_usdo_read(5, 0x1017, 0);
        let ev = decode_usdo(USDO_COB_CLIENT, &payload).unwrap();
        assert_eq!(ev.node_id, 5);
        assert_eq!(ev.index, 0x1017);
        assert_eq!(ev.subindex, 0);
        assert!(ev.is_read);
        assert!(ev.is_request);
    }

    #[test]
    fn encode_decode_write_roundtrip() {
        let payload = encode_usdo_write(3, 0x1400, 1, &[0x01, 0x02, 0x03, 0x04]);
        let ev = decode_usdo(USDO_COB_CLIENT, &payload).unwrap();
        assert_eq!(ev.node_id, 3);
        assert_eq!(ev.index, 0x1400);
        assert_eq!(ev.subindex, 1);
        assert!(!ev.is_read);
        assert_eq!(ev.data, vec![0x01, 0x02, 0x03, 0x04]);
    }

    #[test]
    fn decode_too_short() {
        assert!(decode_usdo(USDO_COB_CLIENT, &[0u8; 7]).is_none());
    }
}
