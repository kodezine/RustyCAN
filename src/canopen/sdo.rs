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

fn interpret_value(raw: &[u8], dtype: Option<&DataType>) -> SdoValue {
    match dtype {
        Some(DataType::Boolean) => {
            SdoValue::Bool(raw.first().copied().unwrap_or(0) != 0)
        }
        Some(DataType::Integer8) => {
            SdoValue::I8(raw.first().copied().unwrap_or(0) as i8)
        }
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
        Some(DataType::Unsigned8) => {
            SdoValue::U8(raw.first().copied().unwrap_or(0))
        }
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
    use crate::eds::types::{AccessType, OdEntry, ObjectDictionary};

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
}
