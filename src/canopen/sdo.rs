use serde::Serialize;

use crate::eds::types::{DataType, ObjectDictionary};

/// Direction from the master's perspective.
#[derive(Debug, Clone, Serialize)]
pub enum SdoDirection {
    /// Master is reading from the node (upload).
    Read,
    /// Master is writing to the node (download).
    Write,
}

/// A typed value decoded from an expedited SDO transfer.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum SdoValue {
    Bool(bool),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    F32(f32),
    F64(f64),
    /// Fallback: raw bytes when the data type is not recognised.
    Bytes(Vec<u8>),
}

impl std::fmt::Display for SdoValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SdoValue::Bool(v) => write!(f, "{v}"),
            SdoValue::I8(v) => write!(f, "{v}"),
            SdoValue::I16(v) => write!(f, "{v}"),
            SdoValue::I32(v) => write!(f, "{v}"),
            SdoValue::I64(v) => write!(f, "{v}"),
            SdoValue::U8(v) => write!(f, "0x{v:02X}"),
            SdoValue::U16(v) => write!(f, "0x{v:04X}"),
            SdoValue::U32(v) => write!(f, "0x{v:08X}"),
            SdoValue::U64(v) => write!(f, "0x{v:016X}"),
            SdoValue::F32(v) => write!(f, "{v:.4}"),
            SdoValue::F64(v) => write!(f, "{v:.6}"),
            SdoValue::Bytes(b) => {
                write!(f, "[")?;
                for (i, byte) in b.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{byte:02X}")?;
                }
                write!(f, "]")
            }
        }
    }
}

/// A decoded (expedited) SDO event.
#[derive(Debug, Clone, Serialize)]
pub struct SdoEvent {
    pub node_id: u8,
    pub direction: SdoDirection,
    pub index: u16,
    pub subindex: u8,
    /// Human-readable name looked up from the object dictionary.
    pub name: String,
    /// Decoded value (absent for upload requests and download acks).
    pub value: Option<SdoValue>,
    /// Abort code if the transfer was aborted.
    pub abort_code: Option<u32>,
}

/// Attempt to decode an SDO frame.
///
/// - `is_response = true`  → COB-ID 0x580+n  (server → client)
/// - `is_response = false` → COB-ID 0x600+n  (client → server)
///
/// Only expedited transfers are decoded; segmented and block transfers return
/// `None`.
pub fn decode_sdo(
    node_id: u8,
    data: &[u8],
    od: &ObjectDictionary,
    is_response: bool,
) -> Option<SdoEvent> {
    if data.len() < 4 {
        return None;
    }

    let cs = data[0];
    let index = u16::from_le_bytes([data[1], data[2]]);
    let subindex = data[3];
    let name = od
        .get(&(index, subindex))
        .map(|e| e.name.clone())
        .unwrap_or_else(|| format!("{index:04X}h/{subindex:02X}"));
    let opt_dtype = od.get(&(index, subindex)).map(|e| &e.data_type);

    if is_response {
        // Server → client (SOB-ID 0x580+n)
        match cs {
            // Expedited upload response: scs=2 (bits 7-5 = 010), e=1, s=1
            // cs = 0x40 | (n<<2) | 0b11
            0x43 | 0x47 | 0x4B | 0x4F => {
                let n_unused = ((cs >> 2) & 0x03) as usize;
                let data_len = 4usize.saturating_sub(n_unused);
                let payload = data.get(4..4 + data_len).unwrap_or(&[]);
                let value = interpret_value(payload, opt_dtype);
                Some(SdoEvent {
                    node_id,
                    direction: SdoDirection::Read,
                    index,
                    subindex,
                    name,
                    value: Some(value),
                    abort_code: None,
                })
            }
            // Download (write) confirmation ack
            0x60 => Some(SdoEvent {
                node_id,
                direction: SdoDirection::Write,
                index,
                subindex,
                name,
                value: None,
                abort_code: None,
            }),
            // Abort transfer
            0x80 => {
                let abort = read_u32_le(data, 4);
                Some(SdoEvent {
                    node_id,
                    direction: SdoDirection::Read,
                    index,
                    subindex,
                    name,
                    value: None,
                    abort_code: Some(abort),
                })
            }
            _ => None,
        }
    } else {
        // Client → server (COB-ID 0x600+n)
        match cs {
            // Upload request (read)
            0x40 => Some(SdoEvent {
                node_id,
                direction: SdoDirection::Read,
                index,
                subindex,
                name,
                value: None,
                abort_code: None,
            }),
            // Expedited download: ccs=1 (bits 7-5 = 001), e=1
            // cs = 0x20 | (n<<2) | (e<<1) | s  where e=1, s=1
            cs if cs & 0xE0 == 0x20 && cs & 0x02 != 0 => {
                let n_unused = ((cs >> 2) & 0x03) as usize;
                let data_len = if cs & 0x01 != 0 {
                    // size indicated
                    4usize.saturating_sub(n_unused)
                } else {
                    // no size indication: consume all 4 remaining bytes
                    4
                };
                let payload = data.get(4..4 + data_len).unwrap_or(&[]);
                let value = interpret_value(payload, opt_dtype);
                Some(SdoEvent {
                    node_id,
                    direction: SdoDirection::Write,
                    index,
                    subindex,
                    name,
                    value: Some(value),
                    abort_code: None,
                })
            }
            // Abort transfer
            0x80 => {
                let abort = read_u32_le(data, 4);
                Some(SdoEvent {
                    node_id,
                    direction: SdoDirection::Write,
                    index,
                    subindex,
                    name,
                    value: None,
                    abort_code: Some(abort),
                })
            }
            _ => None,
        }
    }
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data.get(offset).copied().unwrap_or(0),
        data.get(offset + 1).copied().unwrap_or(0),
        data.get(offset + 2).copied().unwrap_or(0),
        data.get(offset + 3).copied().unwrap_or(0),
    ])
}

// ─── Frame encoding (master-initiated transfers) ──────────────────────────────

/// Build an SDO upload initiate request (master reads from node).
/// COB-ID: 0x600 + node_id.
pub fn encode_upload_request(index: u16, subindex: u8) -> [u8; 8] {
    let [idx_lo, idx_hi] = index.to_le_bytes();
    [0x40, idx_lo, idx_hi, subindex, 0, 0, 0, 0]
}

/// Build an expedited SDO download initiate (master writes ≤4 bytes to node).
/// Returns `None` if `data` is longer than 4 bytes.
/// COB-ID: 0x600 + node_id.
pub fn encode_download_expedited(index: u16, subindex: u8, data: &[u8]) -> Option<[u8; 8]> {
    if data.len() > 4 {
        return None;
    }
    let n = (4 - data.len()) as u8;
    // ccs=1, e=1, s=1 → 0x23 | (n<<2)
    let cs = 0x23u8 | (n << 2);
    let [idx_lo, idx_hi] = index.to_le_bytes();
    let mut frame = [0u8; 8];
    frame[0] = cs;
    frame[1] = idx_lo;
    frame[2] = idx_hi;
    frame[3] = subindex;
    for (i, &b) in data.iter().enumerate() {
        frame[4 + i] = b;
    }
    Some(frame)
}

/// Build a segmented SDO download initiate (master writes >4 bytes to node).
/// `size` is the total byte count of the data to follow.
/// COB-ID: 0x600 + node_id.
pub fn encode_download_initiate_segmented(index: u16, subindex: u8, size: u32) -> [u8; 8] {
    // ccs=1, e=0, s=1 → 0x21
    let [idx_lo, idx_hi] = index.to_le_bytes();
    let [s0, s1, s2, s3] = size.to_le_bytes();
    [0x21, idx_lo, idx_hi, subindex, s0, s1, s2, s3]
}

/// Build an upload segment request (master requests next chunk from node).
/// `toggle` alternates false/true with each segment.
pub fn encode_upload_segment_ack(toggle: bool) -> [u8; 8] {
    // ccs=3 → 0x60 | (toggle << 4)
    let cs = 0x60u8 | (if toggle { 0x10 } else { 0x00 });
    [cs, 0, 0, 0, 0, 0, 0, 0]
}

/// Build a download segment frame carrying up to 7 bytes.
/// `is_last` sets the C-bit to signal this is the final segment.
pub fn encode_download_segment(chunk: &[u8], toggle: bool, is_last: bool) -> [u8; 8] {
    // ccs=0, toggle, n=bytes-not-used, c=last
    let n = (7usize.saturating_sub(chunk.len())) as u8;
    let cs = (if toggle { 0x10u8 } else { 0x00 }) | (n << 1) | (if is_last { 0x01 } else { 0x00 });
    let mut frame = [0u8; 8];
    frame[0] = cs;
    for (i, &b) in chunk.iter().enumerate().take(7) {
        frame[1 + i] = b;
    }
    frame
}

// ─── Incoming segmented response parsers ─────────────────────────────────────

/// Parse a server upload initiate response that uses segmented (non-expedited) transfer.
///
/// Returns `Some(Some(total_size))` when the server indicates size (CS = 0x41),
/// `Some(None)` when it does not (CS = 0x40), or `None` if the frame is not a
/// non-expedited upload initiate response.
pub fn decode_segmented_upload_initiate(data: &[u8]) -> Option<Option<u32>> {
    if data.len() < 8 {
        return None;
    }
    let cs = data[0];
    // scs=2 (bits 7-5 = 010), e=0 (bit 1 = 0): cs & 0xFE must be 0x40
    if cs & 0xFE != 0x40 {
        return None;
    }
    if cs & 0x01 != 0 {
        // s=1: size indicated in bytes 4-7
        let size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        Some(Some(size))
    } else {
        Some(None)
    }
}

/// Parse a server upload segment response (server → client).
///
/// Returns `Some((payload, is_last))` on success, `None` if the command specifier
/// doesn't match an upload segment response.
pub fn decode_upload_segment_response(data: &[u8]) -> Option<(Vec<u8>, bool)> {
    if data.len() < 8 {
        return None;
    }
    let cs = data[0];
    // scs=0 (bits 7-5 = 000), toggle in bit 4, n in bits 3-1, c in bit 0
    if cs & 0xE0 != 0x00 {
        return None;
    }
    let n = ((cs >> 1) & 0x07) as usize; // bytes not used at end
    let is_last = cs & 0x01 != 0;
    let data_len = 7usize.saturating_sub(n);
    let payload = data.get(1..1 + data_len).unwrap_or(&[]).to_vec();
    Some((payload, is_last))
}

/// Return `true` if `data` is a server download initiate acknowledgement (CS = 0x60).
pub fn is_download_initiate_ack(data: &[u8]) -> bool {
    data.first() == Some(&0x60)
}

/// Return `true` if `data` is a server download segment acknowledgement
/// with the expected toggle bit.
pub fn is_download_segment_ack(data: &[u8], expected_toggle: bool) -> bool {
    let expected_cs = 0x20u8 | (if expected_toggle { 0x10 } else { 0x00 });
    data.first() == Some(&expected_cs)
}

// ─── Value encoding utilities ─────────────────────────────────────────────────

/// Encode a string value to bytes according to the given EDS `DataType`.
///
/// - Integers: parsed as decimal or `0x`/`H`-prefixed hex.
/// - Booleans: `"true"` / `"1"` / `"false"` / `"0"`.
/// - `VisibleString`: UTF-8 bytes of the string as-is.
/// - `OctetString` / `Unknown`: forwarded to [`parse_hex_bytes`].
pub fn encode_value_for_type(value_str: &str, dtype: &DataType) -> Result<Vec<u8>, String> {
    match dtype {
        DataType::Boolean => match value_str.trim().to_lowercase().as_str() {
            "true" | "1" => Ok(vec![1]),
            "false" | "0" => Ok(vec![0]),
            _ => Err(format!(
                "Expected boolean (true/1/false/0), got {:?}",
                value_str
            )),
        },
        DataType::Integer8 => value_str
            .trim()
            .parse::<i8>()
            .map(|v| v.to_le_bytes().to_vec())
            .map_err(|_| format!("Expected i8 (-128…127), got {:?}", value_str)),
        DataType::Integer16 => value_str
            .trim()
            .parse::<i16>()
            .map(|v| v.to_le_bytes().to_vec())
            .map_err(|_| format!("Expected i16, got {:?}", value_str)),
        DataType::Integer32 => value_str
            .trim()
            .parse::<i32>()
            .map(|v| v.to_le_bytes().to_vec())
            .map_err(|_| format!("Expected i32, got {:?}", value_str)),
        DataType::Integer64 => value_str
            .trim()
            .parse::<i64>()
            .map(|v| v.to_le_bytes().to_vec())
            .map_err(|_| format!("Expected i64, got {:?}", value_str)),
        DataType::Unsigned8 => parse_uint_auto(value_str, u8::MAX as u64)
            .map(|v| vec![v as u8])
            .map_err(|_| format!("Expected u8 (0…255), got {:?}", value_str)),
        DataType::Unsigned16 => parse_uint_auto(value_str, u16::MAX as u64)
            .map(|v| (v as u16).to_le_bytes().to_vec())
            .map_err(|_| format!("Expected u16 (0…65535), got {:?}", value_str)),
        DataType::Unsigned32 => parse_uint_auto(value_str, u32::MAX as u64)
            .map(|v| (v as u32).to_le_bytes().to_vec())
            .map_err(|_| format!("Expected u32, got {:?}", value_str)),
        DataType::Unsigned64 => parse_uint_auto(value_str, u64::MAX)
            .map(|v| v.to_le_bytes().to_vec())
            .map_err(|_| format!("Expected u64, got {:?}", value_str)),
        DataType::Real32 => value_str
            .trim()
            .parse::<f32>()
            .map(|v| v.to_le_bytes().to_vec())
            .map_err(|_| format!("Expected f32, got {:?}", value_str)),
        DataType::Real64 => value_str
            .trim()
            .parse::<f64>()
            .map(|v| v.to_le_bytes().to_vec())
            .map_err(|_| format!("Expected f64, got {:?}", value_str)),
        DataType::VisibleString => Ok(value_str.as_bytes().to_vec()),
        DataType::OctetString | DataType::Unknown(_) => parse_hex_bytes(value_str),
    }
}

/// Parse a hex-encoded byte string into raw bytes.
///
/// Accepts space-separated pairs (`"01 02 03"`), a continuous hex string
/// (`"010203"`), or a mix. Empty string returns an empty `Vec`.
pub fn parse_hex_bytes(s: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return Ok(vec![]);
    }
    if !cleaned.len().is_multiple_of(2) {
        return Err(format!(
            "Hex string has odd length ({} chars): {:?}",
            cleaned.len(),
            s
        ));
    }
    cleaned
        .as_bytes()
        .chunks(2)
        .map(|chunk| {
            let hex_str = std::str::from_utf8(chunk).map_err(|e| e.to_string())?;
            u8::from_str_radix(hex_str, 16)
                .map_err(|e| format!("Invalid hex byte {:?}: {e}", hex_str))
        })
        .collect()
}

/// Parse an unsigned integer string that may be decimal, `0x`-prefix hex, or
/// `H`/`h`-suffix hex. Returns an error if the value exceeds `max`.
fn parse_uint_auto(s: &str, max: u64) -> Result<u64, ()> {
    let s = s.trim();
    let v: u64 = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).map_err(|_| ())?
    } else if let Some(hex) = s.strip_suffix('H').or_else(|| s.strip_suffix('h')) {
        u64::from_str_radix(hex, 16).map_err(|_| ())?
    } else {
        s.parse::<u64>().map_err(|_| ())?
    };
    if v <= max {
        Ok(v)
    } else {
        Err(())
    }
}

/// Interpret a raw byte buffer as a typed value.
/// Exposed `pub` so the SDO state machine in `session` can decode reassembled
/// segmented transfers using the same logic as expedited transfers.
pub fn interpret_value(raw: &[u8], dtype: Option<&DataType>) -> SdoValue {
    match dtype {
        Some(DataType::Boolean) => SdoValue::Bool(raw.first().copied().unwrap_or(0) != 0),
        Some(DataType::Integer8) => SdoValue::I8(raw.first().copied().unwrap_or(0) as i8),
        Some(DataType::Integer16) if raw.len() >= 2 => {
            SdoValue::I16(i16::from_le_bytes([raw[0], raw[1]]))
        }
        Some(DataType::Integer32) if raw.len() >= 4 => {
            SdoValue::I32(i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
        }
        Some(DataType::Integer64) if raw.len() >= 8 => {
            let b: [u8; 8] = raw[..8].try_into().unwrap();
            SdoValue::I64(i64::from_le_bytes(b))
        }
        Some(DataType::Unsigned8) => SdoValue::U8(raw.first().copied().unwrap_or(0)),
        Some(DataType::Unsigned16) if raw.len() >= 2 => {
            SdoValue::U16(u16::from_le_bytes([raw[0], raw[1]]))
        }
        Some(DataType::Unsigned32) if raw.len() >= 4 => {
            SdoValue::U32(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
        }
        Some(DataType::Unsigned64) if raw.len() >= 8 => {
            let b: [u8; 8] = raw[..8].try_into().unwrap();
            SdoValue::U64(u64::from_le_bytes(b))
        }
        Some(DataType::Real32) if raw.len() >= 4 => {
            SdoValue::F32(f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
        }
        Some(DataType::Real64) if raw.len() >= 8 => {
            let b: [u8; 8] = raw[..8].try_into().unwrap();
            SdoValue::F64(f64::from_le_bytes(b))
        }
        _ => SdoValue::Bytes(raw.to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eds::types::{AccessType, ObjectDictionary, OdEntry};

    fn od_with_u16(index: u16, sub: u8, name: &str) -> ObjectDictionary {
        let mut od = ObjectDictionary::new();
        od.insert(
            (index, sub),
            OdEntry {
                name: name.into(),
                data_type: DataType::Unsigned16,
                access: AccessType::ReadWrite,
                default_value: None,
            },
        );
        od
    }

    #[test]
    fn expedited_upload_response_2bytes() {
        // cs=0x4B  → n=2 unused, so data_len=2
        let data = [0x4B, 0x40, 0x60, 0x00, 0x27, 0x00, 0x00, 0x00];
        let od = od_with_u16(0x6040, 0, "ControlWord");
        let ev = decode_sdo(1, &data, &od, true).unwrap();
        assert_eq!(ev.name, "ControlWord");
        assert!(matches!(ev.value, Some(SdoValue::U16(0x0027))));
    }

    #[test]
    fn upload_request() {
        let data = [0x40, 0x40, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00];
        let od = od_with_u16(0x6040, 0, "ControlWord");
        let ev = decode_sdo(1, &data, &od, false).unwrap();
        assert!(matches!(ev.direction, SdoDirection::Read));
        assert!(ev.value.is_none());
    }

    #[test]
    fn download_ack() {
        let data = [0x60, 0x40, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00];
        let od = od_with_u16(0x6040, 0, "ControlWord");
        let ev = decode_sdo(1, &data, &od, true).unwrap();
        assert!(matches!(ev.direction, SdoDirection::Write));
        assert!(ev.value.is_none());
    }

    #[test]
    fn abort_contains_code() {
        let data = [0x80, 0x40, 0x60, 0x00, 0x00, 0x00, 0x04, 0x06];
        let od = od_with_u16(0x6040, 0, "ControlWord");
        let ev = decode_sdo(1, &data, &od, true).unwrap();
        assert_eq!(ev.abort_code, Some(0x0604_0000));
    }

    #[test]
    fn too_short_returns_none() {
        assert!(decode_sdo(1, &[0x43, 0x00], &ObjectDictionary::new(), true).is_none());
    }

    // ── New: frame encoding ──────────────────────────────────────────────────

    #[test]
    fn encode_upload_request_correct() {
        let frame = encode_upload_request(0x6040, 0);
        assert_eq!(frame[0], 0x40);
        assert_eq!(frame[1], 0x40); // index lo
        assert_eq!(frame[2], 0x60); // index hi
        assert_eq!(frame[3], 0x00); // subindex
        assert_eq!(&frame[4..], &[0, 0, 0, 0]);
    }

    #[test]
    fn encode_download_expedited_1byte() {
        let frame = encode_download_expedited(0x6040, 0, &[0x0F]).unwrap();
        // cs = 0x23 | (3<<2) = 0x23 | 0x0C = 0x2F (3 bytes unused, size indicated)
        assert_eq!(frame[0], 0x2F);
        assert_eq!(frame[1], 0x40);
        assert_eq!(frame[2], 0x60);
        assert_eq!(frame[3], 0x00);
        assert_eq!(frame[4], 0x0F);
    }

    #[test]
    fn encode_download_expedited_4bytes() {
        let frame = encode_download_expedited(0x1000, 0, &[1, 2, 3, 4]).unwrap();
        // cs = 0x23 | (0<<2) = 0x23 (0 bytes unused)
        assert_eq!(frame[0], 0x23);
        assert_eq!(&frame[4..8], &[1, 2, 3, 4]);
    }

    #[test]
    fn encode_download_expedited_rejects_5bytes() {
        assert!(encode_download_expedited(0x1000, 0, &[1, 2, 3, 4, 5]).is_none());
    }

    #[test]
    fn encode_download_initiate_segmented_correct() {
        let frame = encode_download_initiate_segmented(0x1008, 0, 12);
        assert_eq!(frame[0], 0x21); // ccs=1, e=0, s=1
        assert_eq!(u32::from_le_bytes(frame[4..8].try_into().unwrap()), 12);
    }

    #[test]
    fn encode_upload_segment_ack_toggle() {
        let f0 = encode_upload_segment_ack(false);
        assert_eq!(f0[0], 0x60);
        let f1 = encode_upload_segment_ack(true);
        assert_eq!(f1[0], 0x70);
    }

    #[test]
    fn encode_download_segment_last_first() {
        let frame = encode_download_segment(&[1, 2, 3], false, true);
        // n = 7-3 = 4, toggle=false, c=1 → cs = 0x00 | (4<<1) | 1 = 0x09
        assert_eq!(frame[0], 0x09);
        assert_eq!(&frame[1..4], &[1, 2, 3]);
    }

    // ── decode helpers ───────────────────────────────────────────────────────

    #[test]
    fn decode_segmented_upload_initiate_with_size() {
        // CS=0x41 (e=0, s=1), size in bytes 4-7
        let mut data = [0u8; 8];
        data[0] = 0x41;
        data[4..8].copy_from_slice(&42u32.to_le_bytes());
        assert_eq!(decode_segmented_upload_initiate(&data), Some(Some(42)));
    }

    #[test]
    fn decode_segmented_upload_initiate_no_size() {
        let mut data = [0u8; 8];
        data[0] = 0x40; // CS = 0x40 (e=0, s=0)
        assert_eq!(decode_segmented_upload_initiate(&data), Some(None));
    }

    #[test]
    fn decode_segmented_upload_initiate_rejects_expedited() {
        let mut data = [0u8; 8];
        data[0] = 0x43; // expedited upload response — not a segmented initiate
        assert_eq!(decode_segmented_upload_initiate(&data), None);
    }

    #[test]
    fn decode_upload_segment_response_last() {
        // cs=0x01: scs=0, toggle=0, n=0 unused bytes, c=1 (last). 7 data bytes follow.
        let data: [u8; 8] = [0x01, b'H', b'e', b'l', b'l', b'o', b'!', 0x00];
        let (payload, is_last) = decode_upload_segment_response(&data).unwrap();
        assert!(is_last);
        // n=0 unused so all 7 bytes are returned; last byte is 0x00.
        assert_eq!(&payload, b"Hello!\x00");
    }

    #[test]
    fn is_download_initiate_ack_true() {
        assert!(is_download_initiate_ack(&[0x60, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!is_download_initiate_ack(&[0x20, 0, 0, 0, 0, 0, 0, 0]));
    }

    #[test]
    fn is_download_segment_ack_toggle() {
        assert!(is_download_segment_ack(&[0x20, 0, 0, 0, 0, 0, 0, 0], false));
        assert!(is_download_segment_ack(&[0x30, 0, 0, 0, 0, 0, 0, 0], true));
        assert!(!is_download_segment_ack(
            &[0x30, 0, 0, 0, 0, 0, 0, 0],
            false
        ));
    }

    // ── parse_hex_bytes ──────────────────────────────────────────────────────

    #[test]
    fn parse_hex_bytes_spaced() {
        assert_eq!(parse_hex_bytes("01 02 03").unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn parse_hex_bytes_compact() {
        assert_eq!(
            parse_hex_bytes("DEADBEEF").unwrap(),
            vec![0xDE, 0xAD, 0xBE, 0xEF]
        );
    }

    #[test]
    fn parse_hex_bytes_empty() {
        assert_eq!(parse_hex_bytes("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn parse_hex_bytes_odd_length_error() {
        assert!(parse_hex_bytes("0AB").is_err());
    }

    // ── encode_value_for_type ────────────────────────────────────────────────

    #[test]
    fn encode_bool_true() {
        assert_eq!(
            encode_value_for_type("true", &DataType::Boolean).unwrap(),
            vec![1]
        );
        assert_eq!(
            encode_value_for_type("1", &DataType::Boolean).unwrap(),
            vec![1]
        );
    }

    #[test]
    fn encode_bool_false() {
        assert_eq!(
            encode_value_for_type("false", &DataType::Boolean).unwrap(),
            vec![0]
        );
        assert_eq!(
            encode_value_for_type("0", &DataType::Boolean).unwrap(),
            vec![0]
        );
    }

    #[test]
    fn encode_u16_decimal() {
        let bytes = encode_value_for_type("1234", &DataType::Unsigned16).unwrap();
        assert_eq!(u16::from_le_bytes(bytes.try_into().unwrap()), 1234);
    }

    #[test]
    fn encode_u32_hex() {
        let bytes = encode_value_for_type("0x1A2B3C4D", &DataType::Unsigned32).unwrap();
        assert_eq!(u32::from_le_bytes(bytes.try_into().unwrap()), 0x1A2B3C4D);
    }

    #[test]
    fn encode_i32_negative() {
        let bytes = encode_value_for_type("-42", &DataType::Integer32).unwrap();
        assert_eq!(i32::from_le_bytes(bytes.try_into().unwrap()), -42);
    }

    #[test]
    fn encode_f32_value() {
        let bytes = encode_value_for_type("1.5", &DataType::Real32).unwrap();
        let v = f32::from_le_bytes(bytes.try_into().unwrap());
        assert!((v - 1.5f32).abs() < 0.0001);
    }

    #[test]
    fn encode_visible_string() {
        let bytes = encode_value_for_type("hello", &DataType::VisibleString).unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn encode_u8_overflow_error() {
        assert!(encode_value_for_type("256", &DataType::Unsigned8).is_err());
    }
}
