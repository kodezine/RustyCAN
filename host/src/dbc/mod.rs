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

pub use types::{DbcFrameSignals, DbcSignalValue};

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

            values.push(DbcSignalValue {
                signal_name: sig.name.clone(),
                raw_int,
                physical,
                unit: sig.unit.clone(),
                description,
            });
        }

        Some(DbcFrameSignals {
            message_name: msg.name.clone(),
            can_id,
            values,
            source_dbc: source_dbc.clone(),
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
}
