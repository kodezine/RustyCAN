use std::collections::HashMap;

use serde::Serialize;

use crate::eds::parse_default_u32;
use crate::eds::types::{DataType, ObjectDictionary};

/// One signal decoded from a PDO payload.
#[derive(Debug, Clone, Serialize)]
pub struct PdoValue {
    pub signal_name: String,
    pub value: PdoRawValue,
}

impl std::fmt::Display for PdoValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} = {}", self.signal_name, self.value)
    }
}

/// Typed value extracted from a PDO payload byte string.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum PdoRawValue {
    Integer(i64),
    Unsigned(u64),
    Float(f64),
    Text(String),
    /// Fallback for non-standard bit widths.
    Bytes(Vec<u8>),
}

impl std::fmt::Display for PdoRawValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdoRawValue::Integer(v) => write!(f, "{v}"),
            PdoRawValue::Unsigned(v) => write!(f, "{v}"),
            PdoRawValue::Float(v) => write!(f, "{v:.4}"),
            PdoRawValue::Text(s) => write!(f, "{s}"),
            PdoRawValue::Bytes(b) => {
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

/// Layout of one signal within a PDO payload.
#[derive(Debug, Clone)]
pub struct PdoSignal {
    pub name: String,
    /// Bit offset from byte 0 of the payload (CANopen: little-endian packed).
    pub bit_offset: usize,
    pub bit_length: usize,
    pub data_type: DataType,
}

/// Holds all TPDO decoders for a single node, keyed by COB-ID.
pub struct PdoDecoder {
    /// COB-ID → (EDS 1-based pdo_num, ordered list of signals).
    pub mappings: HashMap<u16, (u8, Vec<PdoSignal>)>,
}

impl PdoDecoder {
    /// Build a `PdoDecoder` from an EDS object dictionary for `node_id`.
    ///
    /// Reads TPDO communication parameters (0x1800–0x1803) and mapping
    /// objects (0x1A00–0x1A03). RPDO entries (0x1400/0x1600) are
    /// similarly processed.
    pub fn from_od(node_id: u8, od: &ObjectDictionary) -> Self {
        let mut mappings = HashMap::new();

        // Process both TPDO (0x1800/0x1A00) and RPDO (0x1400/0x1600).
        let pdo_ranges: &[(u16, u16)] = &[
            (0x1800, 0x1A00), // TPDO
            (0x1400, 0x1600), // RPDO
        ];

        for &(comm_base, map_base) in pdo_ranges {
            for pdo_num in 0..4u16 {
                let comm_idx = comm_base + pdo_num;
                let map_idx = map_base + pdo_num;

                // Determine COB-ID.  Subindex 1 of the comm params holds the
                // 32-bit COB-ID; the MSB (bit 31) is the "invalid/disabled" flag.
                let cob_id_raw = od
                    .get(&(comm_idx, 1))
                    .and_then(|e| e.default_value.as_deref())
                    .and_then(parse_default_u32);

                // If the invalid flag (bit 31) is set the PDO is disabled.
                if let Some(raw) = cob_id_raw {
                    if raw & 0x8000_0000 != 0 {
                        continue;
                    }
                }

                // Fall back to the default COB-ID assignment if not in EDS.
                let cob_id = match cob_id_raw {
                    Some(v) => (v & 0x7FF) as u16,
                    None => {
                        // Default assignment: TPDO1 = 0x180 + node, etc.
                        let base: u16 = if comm_base == 0x1800 { 0x180 } else { 0x200 };
                        base + node_id as u16 + pdo_num * 0x100
                    }
                };

                // Number of mapped objects (subindex 0 of the mapping object).
                let num_mapped = od
                    .get(&(map_idx, 0))
                    .and_then(|e| e.default_value.as_deref())
                    .and_then(parse_default_u32)
                    .unwrap_or(0) as u8;

                if num_mapped == 0 {
                    continue;
                }

                let mut bit_offset = 0usize;
                let mut signals = Vec::new();

                for sub in 1..=num_mapped {
                    let mapping_val = od
                        .get(&(map_idx, sub))
                        .and_then(|e| e.default_value.as_deref())
                        .and_then(parse_default_u32);

                    if let Some(mv) = mapping_val {
                        let obj_index = (mv >> 16) as u16;
                        let obj_sub = ((mv >> 8) & 0xFF) as u8;
                        let bit_length = (mv & 0xFF) as usize;

                        let (name, dtype) = od
                            .get(&(obj_index, obj_sub))
                            .map(|e| (e.name.clone(), e.data_type.clone()))
                            .unwrap_or_else(|| {
                                (
                                    format!("{obj_index:04X}h/{obj_sub:02X}"),
                                    DataType::Unknown(0),
                                )
                            });

                        signals.push(PdoSignal {
                            name,
                            bit_offset,
                            bit_length,
                            data_type: dtype,
                        });
                        bit_offset += bit_length;
                    }
                }

                if !signals.is_empty() {
                    mappings.insert(cob_id, ((pdo_num as u8) + 1, signals));
                }
            }
        }

        PdoDecoder { mappings }
    }

    /// Decode a PDO data payload for the given COB-ID.
    ///
    /// Returns `None` if COB-ID is not in the mapping table.
    pub fn decode(&self, cob_id: u16, data: &[u8]) -> Option<Vec<PdoValue>> {
        let (_, signals) = self.mappings.get(&cob_id)?;
        let values = signals
            .iter()
            .map(|sig| PdoValue {
                signal_name: sig.name.clone(),
                value: extract_bits(data, sig.bit_offset, sig.bit_length, &sig.data_type),
            })
            .collect();
        Some(values)
    }

    /// Return the EDS-derived 1-based PDO number for a given COB-ID, if known.
    pub fn pdo_num_for_cob_id(&self, cob_id: u16) -> Option<u8> {
        self.mappings.get(&cob_id).map(|(pdo_num, _)| *pdo_num)
    }
}

/// Extract `bit_length` bits starting at `bit_offset` from `data` and
/// interpret them as the given `DataType`.
///
/// CANopen PDO signals use little-endian byte order and are byte-aligned for
/// standard types (8/16/32/64-bit).  Non-byte-aligned widths fall through to
/// the raw-bytes variant.
fn extract_bits(
    data: &[u8],
    bit_offset: usize,
    bit_length: usize,
    dtype: &DataType,
) -> PdoRawValue {
    if bit_length == 0 {
        return PdoRawValue::Bytes(vec![]);
    }

    let byte_offset = bit_offset / 8;
    if byte_offset >= data.len() {
        return PdoRawValue::Bytes(vec![]);
    }

    // VisibleString / OctetString: treat bytes directly as text/bytes.
    if matches!(dtype, DataType::VisibleString | DataType::OctetString) {
        let byte_len = bit_length.div_ceil(8);
        let end = (byte_offset + byte_len).min(data.len());
        let bytes = &data[byte_offset..end];
        return match std::str::from_utf8(bytes) {
            Ok(s) => PdoRawValue::Text(s.trim_end_matches('\0').to_string()),
            Err(_) => PdoRawValue::Bytes(bytes.to_vec()),
        };
    }

    let rest = &data[byte_offset..];

    match bit_length {
        8 if !rest.is_empty() => match dtype {
            DataType::Integer8 => PdoRawValue::Integer(rest[0] as i8 as i64),
            DataType::Boolean => PdoRawValue::Unsigned(rest[0] as u64 & 1),
            _ => PdoRawValue::Unsigned(rest[0] as u64),
        },
        16 if rest.len() >= 2 => {
            let v = u16::from_le_bytes([rest[0], rest[1]]);
            match dtype {
                DataType::Integer16 => PdoRawValue::Integer(v as i16 as i64),
                _ => PdoRawValue::Unsigned(v as u64),
            }
        }
        32 if rest.len() >= 4 => {
            let v = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
            match dtype {
                DataType::Integer32 => PdoRawValue::Integer(v as i32 as i64),
                DataType::Real32 => PdoRawValue::Float(f32::from_bits(v) as f64),
                _ => PdoRawValue::Unsigned(v as u64),
            }
        }
        64 if rest.len() >= 8 => {
            let v = u64::from_le_bytes(rest[..8].try_into().unwrap());
            match dtype {
                DataType::Integer64 => PdoRawValue::Integer(v as i64),
                DataType::Real64 => PdoRawValue::Float(f64::from_bits(v)),
                _ => PdoRawValue::Unsigned(v),
            }
        }
        _ => {
            // Generic: copy byte range
            let byte_len = bit_length.div_ceil(8);
            let end = (byte_offset + byte_len).min(data.len());
            PdoRawValue::Bytes(data[byte_offset..end].to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eds::types::{AccessType, OdEntry};

    fn make_od_entry(name: &str, dtype: DataType) -> OdEntry {
        OdEntry {
            name: name.into(),
            data_type: dtype,
            access: AccessType::ReadOnly,
            default_value: None,
        }
    }

    #[test]
    fn extract_u16_le() {
        // 0x1234 stored as [0x34, 0x12] in little-endian
        let data = [0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let v = extract_bits(&data, 0, 16, &DataType::Unsigned16);
        assert!(matches!(v, PdoRawValue::Unsigned(0x1234)));
    }

    #[test]
    fn extract_i16_negative() {
        // -1 as i16 LE = [0xFF, 0xFF]
        let data = [0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let v = extract_bits(&data, 0, 16, &DataType::Integer16);
        assert!(matches!(v, PdoRawValue::Integer(-1)));
    }

    #[test]
    fn extract_u8_at_offset() {
        let data = [0x00, 0xAB, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let v = extract_bits(&data, 8, 8, &DataType::Unsigned8);
        assert!(matches!(v, PdoRawValue::Unsigned(0xAB)));
    }

    #[test]
    fn extract_f32() {
        let f: f32 = 1.5;
        let bytes = f.to_le_bytes();
        let mut data = [0u8; 8];
        data[..4].copy_from_slice(&bytes);
        let v = extract_bits(&data, 0, 32, &DataType::Real32);
        match v {
            PdoRawValue::Float(val) => assert!((val - 1.5).abs() < 1e-6),
            _ => panic!("expected Float"),
        }
    }

    #[test]
    fn decoder_unknown_cob_returns_none() {
        let decoder = PdoDecoder {
            mappings: HashMap::new(),
        };
        assert!(decoder.decode(0x181, &[0; 8]).is_none());
    }

    #[test]
    fn decoder_from_od_simple() {
        // Build a minimal OD: TPDO1 comm (0x1800), map (0x1A00)
        // Map 2 signals: velocity (u16 @ 0x6044/0) and torque (i16 @ 0x6071/0)
        let mut od = ObjectDictionary::new();

        // TPDO1 comm params
        od.insert(
            (0x1800, 1),
            OdEntry {
                name: "COB-ID TPDO1".into(),
                data_type: DataType::Unsigned32,
                access: AccessType::ReadWrite,
                default_value: Some("0x00000181".into()), // node 1
            },
        );
        od.insert(
            (0x1A00, 0),
            OdEntry {
                name: "NumberOfMappedObjects".into(),
                data_type: DataType::Unsigned8,
                access: AccessType::ReadWrite,
                default_value: Some("2".into()),
            },
        );
        // Signal 1: 0x6044 sub0, 16 bits
        od.insert(
            (0x1A00, 1),
            OdEntry {
                name: "Mapped1".into(),
                data_type: DataType::Unsigned32,
                access: AccessType::ReadWrite,
                default_value: Some("0x60440010".into()), // idx=6044, sub=0, 16 bits
            },
        );
        od.insert(
            (0x6044, 0),
            make_od_entry("VelocityActual", DataType::Unsigned16),
        );
        // Signal 2: 0x6071 sub0, 16 bits
        od.insert(
            (0x1A00, 2),
            OdEntry {
                name: "Mapped2".into(),
                data_type: DataType::Unsigned32,
                access: AccessType::ReadWrite,
                default_value: Some("0x60710010".into()), // idx=6071, sub=0, 16 bits
            },
        );
        od.insert(
            (0x6071, 0),
            make_od_entry("TargetTorque", DataType::Integer16),
        );

        let decoder = PdoDecoder::from_od(1, &od);
        assert!(decoder.mappings.contains_key(&0x181), "TPDO1 missing");

        let data = [0x34, 0x12, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00];
        let values = decoder.decode(0x181, &data).unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].signal_name, "VelocityActual");
        assert!(matches!(values[0].value, PdoRawValue::Unsigned(0x1234)));
        assert_eq!(values[1].signal_name, "TargetTorque");
        assert!(matches!(values[1].value, PdoRawValue::Integer(-1)));
    }
}
