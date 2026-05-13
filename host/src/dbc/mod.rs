//! DBC (CAN database) file parser and signal decoder.
//!
//! Provides [`DbcDatabase`], which loads a `.dbc` file (with UTF-8 or
//! CP-1252 encoding) and decodes signals from raw CAN frame payloads.
//!
//! Both Intel (little-endian) and Motorola (big-endian) byte orders are
//! supported.  Multiplexed signals are skipped; all other signals are decoded.
//!
//! Value descriptions (`VAL_`) are precomputed into the database at load time
//! and surfaced as `description` in [`DbcSignalValue`].

pub mod types;

use std::collections::HashMap;
use std::path::Path;

use can_dbc::{ByteOrder, Dbc, MessageId, MultiplexIndicator, ValueDescription, ValueType};

pub use types::{DbcByteOrder, DbcFrameSignals, DbcSignalDef, DbcSignalValue, DbcValueType};

// ─── Public database type ─────────────────────────────────────────────────────

/// Loaded DBC database ready for per-frame decoding.
pub struct DbcDatabase {
    /// Normalized CAN ID → list of (source_name, message).
    ///
    /// Multiple DBCs can define the same CAN ID; all are stored and the first
    /// match is used for decoding. Key: standard IDs stored as-is (u16 cast
    /// to u32), extended IDs without the bit-31 EFF marker.
    messages: HashMap<u32, Vec<(String, can_dbc::Message)>>,
    /// `(can_id, signal_name)` → `raw_i64 → description`.
    val_descs: HashMap<(u32, String), HashMap<i64, String>>,
}

impl DbcDatabase {
    /// Decode all non-multiplexed signals from `data` for the given `can_id`.
    ///
    /// Returns `None` if `can_id` does not match any message in the database.
    /// If multiple DBCs define the same ID, the first one is used.
    pub fn decode_frame(&self, can_id: u32, data: &[u8]) -> Option<DbcFrameSignals> {
        let entries = self.messages.get(&can_id)?;
        let (source_dbc, msg) = entries.first()?;
        let mut values = Vec::with_capacity(msg.signals.len());

        for sig in &msg.signals {
            // Skip multiplexed signals for now (decode Plain and Multiplexor).
            if matches!(
                sig.multiplexer_indicator,
                MultiplexIndicator::MultiplexedSignal(_)
            ) {
                continue;
            }

            let raw_u64 = match sig.byte_order {
                ByteOrder::LittleEndian => extract_intel(data, sig.start_bit, sig.size),
                ByteOrder::BigEndian => extract_motorola(data, sig.start_bit, sig.size),
            };

            let raw_int: i64 = match sig.value_type {
                ValueType::Signed => to_signed(raw_u64, sig.size),
                ValueType::Unsigned => raw_u64 as i64,
            };

            let physical = raw_int as f64 * sig.factor + sig.offset;

            // Look up the value description for this raw integer value.
            let description = self
                .val_descs
                .get(&(can_id, sig.name.clone()))
                .and_then(|map| map.get(&raw_int))
                .cloned();

            let encoding_def = Some(DbcSignalDef {
                start_bit: sig.start_bit,
                bit_size: sig.size,
                byte_order: match sig.byte_order {
                    ByteOrder::LittleEndian => DbcByteOrder::LittleEndian,
                    ByteOrder::BigEndian => DbcByteOrder::BigEndian,
                },
                value_type: match sig.value_type {
                    ValueType::Signed => DbcValueType::Signed,
                    ValueType::Unsigned => DbcValueType::Unsigned,
                },
                factor: sig.factor,
                offset: sig.offset,
                dlc: msg.size as u8,
            });

            values.push(DbcSignalValue {
                signal_name: sig.name.clone(),
                raw_int,
                physical,
                unit: sig.unit.clone(),
                description,
                encoding_def,
            });
        }

        Some(DbcFrameSignals {
            message_name: msg.name.clone(),
            can_id,
            values,
            source_dbc: source_dbc.clone(),
            raw_data: data.to_vec(),
        })
    }
}

// ─── Load function ────────────────────────────────────────────────────────────

/// Load a DBC file from `path`.
///
/// Reads the file as raw bytes, tries UTF-8 first, then falls back to
/// CP-1252 (common for DBC files produced by Vector toolchain).
///
/// Returns an error string on parse failure.
pub fn load_dbc(path: &Path) -> Result<DbcDatabase, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("Cannot read DBC file {}: {e}", path.display()))?;

    let text = match String::from_utf8(bytes.clone()) {
        Ok(s) => s,
        Err(_) => can_dbc::decode_cp1252(&bytes)
            .map(|cow| cow.into_owned())
            .unwrap_or_else(|| String::from_utf8_lossy(&bytes).into_owned()),
    };

    let dbc = Dbc::try_from(text.as_str())
        .map_err(|e| format!("Failed to parse DBC file {}: {e:?}", path.display()))?;

    let source_name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_string());

    // ── Build message lookup ─────────────────────────────────────────────────
    let mut messages: HashMap<u32, Vec<(String, can_dbc::Message)>> = HashMap::new();
    for msg in &dbc.messages {
        let key = msg_id_to_key(msg.id);
        messages
            .entry(key)
            .or_default()
            .push((source_name.clone(), msg.clone()));
    }

    // ── Build value-description lookup ───────────────────────────────────────
    let mut val_descs: HashMap<(u32, String), HashMap<i64, String>> = HashMap::new();
    for vd in &dbc.value_descriptions {
        if let ValueDescription::Signal {
            message_id,
            name,
            value_descriptions,
        } = vd
        {
            let can_id = msg_id_to_key(*message_id);
            let inner = val_descs.entry((can_id, name.clone())).or_default();
            for entry in value_descriptions {
                inner.insert(entry.id, entry.description.clone());
            }
        }
    }

    Ok(DbcDatabase {
        messages,
        val_descs,
    })
}

// ─── Encoding ─────────────────────────────────────────────────────────────────

impl DbcDatabase {
    /// Return the DLC (payload length in bytes) of the message with `can_id`,
    /// or `None` if the ID is not in this database.
    pub fn message_dlc(&self, can_id: u32) -> Option<u8> {
        let (_, msg) = self.messages.get(&can_id)?.first()?;
        Some(msg.size as u8)
    }

    /// Encode a physical value for one signal into a copy of `current_bytes`.
    ///
    /// `current_bytes` must be at least `dlc` bytes long; the bits belonging to
    /// other signals in the same message are preserved unchanged.
    ///
    /// Returns `None` if `can_id` or `signal_name` is not in the database.
    pub fn encode_signal(
        &self,
        can_id: u32,
        signal_name: &str,
        physical: f64,
        current_bytes: &[u8],
    ) -> Option<Vec<u8>> {
        let (_, msg) = self.messages.get(&can_id)?.first()?;
        let sig = msg.signals.iter().find(|s| s.name == signal_name)?;

        let dlc = msg.size as usize;
        let mut data: Vec<u8> = current_bytes
            .iter()
            .copied()
            .chain(std::iter::repeat(0))
            .take(dlc)
            .collect();

        // Inverse of physical = raw * factor + offset
        let factor = sig.factor;
        let offset = sig.offset;
        let raw_f = if factor == 0.0 {
            0.0
        } else {
            (physical - offset) / factor
        };

        // Clamp to representable bit-field range.
        let max_unsigned = if sig.size >= 64 {
            u64::MAX
        } else {
            (1u64 << sig.size) - 1
        };
        let raw_u64: u64 = match sig.value_type {
            ValueType::Signed => {
                let min_s = -(1i64 << (sig.size - 1).min(63));
                let max_s = (1i64 << (sig.size - 1).min(63)) - 1;
                let clamped = (raw_f.round() as i64).clamp(min_s, max_s);
                // Reinterpret as unsigned twos-complement for packing.
                clamped as u64 & max_unsigned
            }
            ValueType::Unsigned => (raw_f.round() as u64).min(max_unsigned),
        };

        match sig.byte_order {
            ByteOrder::LittleEndian => pack_intel(&mut data, sig.start_bit, sig.size, raw_u64),
            ByteOrder::BigEndian => pack_motorola(&mut data, sig.start_bit, sig.size, raw_u64),
        }

        Some(data)
    }
}

/// Pack `length` bits of `raw` into `data` using Intel (little-endian) byte order.
///
/// Mirrors [`extract_intel`]: `start_bit` is the LSBit position.  Bits that
/// belong to other signals are left unchanged.
fn pack_intel(data: &mut [u8], start_bit: u64, length: u64, raw: u64) {
    for i in 0..length {
        let bit_pos = start_bit + i;
        let byte_idx = (bit_pos / 8) as usize;
        let bit_in_byte = (bit_pos % 8) as u8;
        if byte_idx >= data.len() {
            break;
        }
        let bit_val = ((raw >> i) & 1) as u8;
        data[byte_idx] &= !(1 << bit_in_byte);
        data[byte_idx] |= bit_val << bit_in_byte;
    }
}

/// Pack `length` bits of `raw` into `data` using Motorola (big-endian) byte order.
///
/// Mirrors [`extract_motorola`]: `start_bit` is the MSBit position in DBC
/// numbering.  Other bits are left unchanged.
fn pack_motorola(data: &mut [u8], start_bit: u64, length: u64, raw: u64) {
    if length == 0 {
        return;
    }
    let mut bit_pos = start_bit as usize;
    for i in 0..length as usize {
        let byte_idx = bit_pos / 8;
        let bit_in_byte = bit_pos % 8;
        if byte_idx >= data.len() {
            break;
        }
        // MSBit first: bit i of the traversal → position (length-1-i) in raw.
        let bit_val = ((raw >> (length as usize - 1 - i)) & 1) as u8;
        data[byte_idx] &= !(1 << bit_in_byte);
        data[byte_idx] |= bit_val << bit_in_byte;
        // Same traversal order as extract_motorola.
        if bit_in_byte == 0 {
            bit_pos = (byte_idx + 1) * 8 + 7;
        } else {
            bit_pos -= 1;
        }
    }
}

/// Merge multiple DBC databases into one.
///
/// When the same CAN ID appears in multiple DBCs, all definitions are preserved
/// in the order provided. The first match is used during decode_frame().
pub fn merge_databases(databases: Vec<DbcDatabase>) -> Result<DbcDatabase, String> {
    let mut merged_messages: HashMap<u32, Vec<(String, can_dbc::Message)>> = HashMap::new();
    let mut merged_val_descs: HashMap<(u32, String), HashMap<i64, String>> = HashMap::new();

    for db in databases {
        // Merge messages - preserve all entries for overlapping IDs
        for (can_id, entries) in db.messages {
            merged_messages.entry(can_id).or_default().extend(entries);
        }

        // Merge value descriptions - first-wins for conflicts
        for (key, desc_map) in db.val_descs {
            merged_val_descs.entry(key).or_default().extend(desc_map);
        }
    }

    Ok(DbcDatabase {
        messages: merged_messages,
        val_descs: merged_val_descs,
    })
}

// ─── ID normalization ─────────────────────────────────────────────────────────

/// Convert a DBC `MessageId` to the u32 key used in our lookup table.
///
/// Standard IDs are stored as their u16 value cast to u32.
/// Extended IDs are stored without the EFF bit (i.e., the 29-bit value).
fn msg_id_to_key(id: MessageId) -> u32 {
    match id {
        MessageId::Standard(s) => s as u32,
        MessageId::Extended(e) => e & 0x1FFF_FFFF,
    }
}

// ─── Bit extraction ───────────────────────────────────────────────────────────

/// Extract `length` bits from `data` using Intel (little-endian) byte order.
///
/// `start_bit` is the LSBit position in a flat bit numbering where bit 0 is
/// the LSB of byte 0.
fn extract_intel(data: &[u8], start_bit: u64, length: u64) -> u64 {
    let mut raw: u64 = 0;
    for i in 0..length {
        let bit_pos = start_bit + i;
        let byte_idx = (bit_pos / 8) as usize;
        let bit_in_byte = (bit_pos % 8) as u8;
        if byte_idx >= data.len() {
            break;
        }
        let bit_val = ((data[byte_idx] >> bit_in_byte) & 1) as u64;
        raw |= bit_val << i;
    }
    raw
}

/// Extract `length` bits from `data` using Motorola (big-endian) byte order.
///
/// `start_bit` is the MSBit position using the DBC bit numbering:
///   bit N → byte N/8, bit (N%8) within the byte (bit 0 = LSB, bit 7 = MSB).
///
/// Bits are extracted MSBit-first; within each byte the bit_in_byte decrements
/// from start (or 7) down to 0, then wraps to bit 7 of the next byte.
fn extract_motorola(data: &[u8], start_bit: u64, length: u64) -> u64 {
    if length == 0 {
        return 0;
    }
    let mut raw: u64 = 0;
    let mut bit_pos = start_bit as usize;

    for i in 0..length as usize {
        let byte_idx = bit_pos / 8;
        let bit_in_byte = bit_pos % 8;
        if byte_idx >= data.len() {
            break;
        }
        let bit_val = ((data[byte_idx] >> bit_in_byte) & 1) as u64;
        // Place MSBit first in the output, so bit i maps to position (length-1-i).
        raw |= bit_val << (length as usize - 1 - i);
        // Advance to next bit: decrement within byte, then jump to next byte's MSBit.
        if bit_in_byte == 0 {
            bit_pos = (byte_idx + 1) * 8 + 7;
        } else {
            bit_pos -= 1;
        }
    }
    raw
}

/// Sign-extend a `length`-bit two's-complement unsigned value to `i64`.
fn to_signed(raw: u64, length: u64) -> i64 {
    if length == 0 || length >= 64 {
        return raw as i64;
    }
    let sign_bit = 1u64 << (length - 1);
    if raw & sign_bit != 0 {
        // Fill the upper bits with 1s.
        let mask = !((sign_bit << 1).wrapping_sub(1));
        (raw | mask) as i64
    } else {
        raw as i64
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_intel_single_byte() {
        // 0xAB = 0b10101011, start=0, len=8 → raw = 0xAB
        assert_eq!(extract_intel(&[0xABu8], 0, 8), 0xAB);
    }

    #[test]
    fn extract_intel_nibble() {
        // lower 4 bits of 0xAB = 0x0B
        assert_eq!(extract_intel(&[0xABu8], 0, 4), 0x0B);
    }

    #[test]
    fn extract_intel_cross_byte() {
        // bytes: [0x00, 0x01], start=8, len=8 → 0x01
        assert_eq!(extract_intel(&[0x00u8, 0x01], 8, 8), 0x01);
    }

    #[test]
    fn extract_motorola_single_byte() {
        // 0xAB = 0b10101011, MSBit=7, len=8 → raw = 0xAB
        assert_eq!(extract_motorola(&[0xABu8], 7, 8), 0xAB);
    }

    #[test]
    fn to_signed_positive() {
        assert_eq!(to_signed(0x7F, 8), 127);
    }

    #[test]
    fn to_signed_negative() {
        assert_eq!(to_signed(0xFF, 8), -1);
    }

    #[test]
    fn to_signed_8bit_minus128() {
        assert_eq!(to_signed(0x80, 8), -128);
    }

    // ─── pack / encode tests ─────────────────────────────────────────────────

    #[test]
    fn pack_intel_roundtrip_single_byte() {
        let mut data = [0u8; 1];
        pack_intel(&mut data, 0, 8, 0xAB);
        assert_eq!(extract_intel(&data, 0, 8), 0xAB);
    }

    #[test]
    fn pack_intel_preserves_other_bits() {
        // Put 0xFF in byte 0, then overwrite only the lower nibble with 0x5.
        let mut data = [0xFFu8; 1];
        pack_intel(&mut data, 0, 4, 0x5);
        // Upper nibble (bits 4-7) must stay 0xF.
        assert_eq!(data[0], 0xF5);
    }

    #[test]
    fn pack_motorola_roundtrip_single_byte() {
        let mut data = [0u8; 1];
        pack_motorola(&mut data, 7, 8, 0xAB);
        assert_eq!(extract_motorola(&data, 7, 8), 0xAB);
    }

    #[test]
    fn encode_signal_intel_le_factor_offset() {
        use std::path::Path;
        // Load the integration fixture and round-trip HSP_ABS_POSITION_M.
        // physical = 34.2, factor = 0.0342, offset = 0  → raw = 1000
        let db = super::load_dbc(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/sample_bus.dbc"
        )))
        .expect("fixture not found");
        let current = [0u8; 8];
        let encoded = db
            .encode_signal(105, "HSP_ABS_POSITION_M", 34.2, &current)
            .expect("encode_signal returned None");
        // raw 1000 = 0x03E8 → bytes [0xE8, 0x03, ...]
        assert_eq!(encoded[0], 0xE8);
        assert_eq!(encoded[1], 0x03);
    }

    #[test]
    fn encode_signal_preserves_adjacent_bits() {
        use std::path::Path;
        // BP_DIRECTION_GET_M: CAN ID 57, start_bit=24, length=8.
        // Pre-fill byte 3 with something else, encode raw=1 ("True"),
        // and verify only byte 3 changed.
        let db = super::load_dbc(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/sample_bus.dbc"
        )))
        .expect("fixture not found");
        let current = [0xAA, 0xBB, 0xCC, 0x00, 0x11, 0x22, 0x33, 0x44];
        let encoded = db
            .encode_signal(57, "BP_DIRECTION_GET_M", 1.0, &current)
            .expect("encode_signal returned None");
        assert_eq!(encoded[3], 0x01);
        // Non-overlapping bytes are unchanged.
        assert_eq!(encoded[0], 0xAA);
        assert_eq!(encoded[4], 0x11);
    }
}
