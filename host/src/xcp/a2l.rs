//! Minimal A2L (ASAM MCD-2 MC) parser for measurement / characteristic lookup.
//!
//! This is intentionally a **small subset** of A2L: it extracts `MEASUREMENT`
//! and `CHARACTERISTIC` objects and exposes address → (name, datatype) lookup so
//! XCP UPLOAD / DAQ values can be labelled. It performs **no** compu-method
//! (engineering-unit) conversion — values are decoded to their raw numeric form
//! only, matching the v1 scope.
//!
//! The parser is a forgiving tokenizer/state-machine, not a full grammar: it
//! tolerates unknown blocks and keywords, so real-world A2L files parse without
//! needing every optional element modelled.

use std::collections::HashMap;

/// A2L base datatype (`Datatype` in MEASUREMENT, or via RECORD_LAYOUT for
/// CHARACTERISTIC — only the directly-stated MEASUREMENT datatype is modelled).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A2lType {
    UByte,
    SByte,
    UWord,
    SWord,
    ULong,
    SLong,
    AUint64,
    AInt64,
    Float32,
    Float64,
    Unknown,
}

impl A2lType {
    /// Parse an A2L datatype keyword (case-insensitive).
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_uppercase().as_str() {
            "UBYTE" => A2lType::UByte,
            "SBYTE" => A2lType::SByte,
            "UWORD" => A2lType::UWord,
            "SWORD" => A2lType::SWord,
            "ULONG" => A2lType::ULong,
            "SLONG" => A2lType::SLong,
            "A_UINT64" => A2lType::AUint64,
            "A_INT64" => A2lType::AInt64,
            "FLOAT32_IEEE" => A2lType::Float32,
            "FLOAT64_IEEE" => A2lType::Float64,
            _ => A2lType::Unknown,
        }
    }

    /// Size in bytes (0 for [`A2lType::Unknown`]).
    pub fn size(self) -> usize {
        match self {
            A2lType::UByte | A2lType::SByte => 1,
            A2lType::UWord | A2lType::SWord => 2,
            A2lType::ULong | A2lType::SLong | A2lType::Float32 => 4,
            A2lType::AUint64 | A2lType::AInt64 | A2lType::Float64 => 8,
            A2lType::Unknown => 0,
        }
    }
}

/// A decoded raw A2L value (no engineering-unit conversion).
#[derive(Debug, Clone, PartialEq)]
pub enum A2lValue {
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    /// Fallback when the datatype is unknown or bytes are insufficient.
    Raw(Vec<u8>),
}

impl std::fmt::Display for A2lValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            A2lValue::Signed(v) => write!(f, "{v}"),
            A2lValue::Unsigned(v) => write!(f, "{v}"),
            A2lValue::Float(v) => write!(f, "{v}"),
            A2lValue::Raw(b) => {
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

/// A measurement or characteristic object with a resolved ECU address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A2lObject {
    /// Object name (identifier).
    pub name: String,
    /// ECU memory address.
    pub address: u32,
    /// Datatype, when known (CHARACTERISTIC datatypes are often deferred to a
    /// RECORD_LAYOUT and left [`A2lType::Unknown`]).
    pub datatype: A2lType,
    /// `true` for CHARACTERISTIC (calibration), `false` for MEASUREMENT.
    pub is_characteristic: bool,
}

/// Parsed A2L database: objects plus an address index.
#[derive(Debug, Clone, Default)]
pub struct A2lDatabase {
    objects: Vec<A2lObject>,
    by_address: HashMap<u32, usize>,
}

impl A2lDatabase {
    /// Parse an A2L source string into a database.
    ///
    /// Malformed individual objects are skipped; parsing never fails outright.
    pub fn parse(src: &str) -> Self {
        let tokens = tokenize(src);
        let mut db = A2lDatabase::default();
        let mut i = 0;
        while i < tokens.len() {
            if tokens[i] == "/begin" && i + 1 < tokens.len() {
                match tokens[i + 1].as_str() {
                    "MEASUREMENT" => {
                        if let Some((obj, next)) = parse_measurement(&tokens, i + 2) {
                            db.insert(obj);
                            i = next;
                            continue;
                        }
                    }
                    "CHARACTERISTIC" => {
                        if let Some((obj, next)) = parse_characteristic(&tokens, i + 2) {
                            db.insert(obj);
                            i = next;
                            continue;
                        }
                    }
                    _ => {}
                }
            }
            i += 1;
        }
        db
    }

    fn insert(&mut self, obj: A2lObject) {
        let idx = self.objects.len();
        // First definition of an address wins (MEASUREMENT precedes duplicates).
        self.by_address.entry(obj.address).or_insert(idx);
        self.objects.push(obj);
    }

    /// Number of parsed objects.
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Whether the database has no objects.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// All parsed objects.
    pub fn objects(&self) -> &[A2lObject] {
        &self.objects
    }

    /// Look up the object registered at `address`.
    pub fn object_at(&self, address: u32) -> Option<&A2lObject> {
        self.by_address.get(&address).map(|&i| &self.objects[i])
    }

    /// Convenience: name registered at `address`.
    pub fn name_for_address(&self, address: u32) -> Option<&str> {
        self.object_at(address).map(|o| o.name.as_str())
    }

    /// Decode `raw` bytes at `address` into a raw [`A2lValue`] using the object's
    /// datatype and the given `byte_order`. Returns `None` if the address is
    /// unknown; falls back to [`A2lValue::Raw`] for unknown/oversized datatypes.
    pub fn decode_at(
        &self,
        address: u32,
        raw: &[u8],
        byte_order: super::ByteOrder,
    ) -> Option<A2lValue> {
        let obj = self.object_at(address)?;
        Some(decode_value(obj.datatype, raw, byte_order))
    }
}

/// Decode raw bytes into a typed raw value per datatype and byte order.
pub fn decode_value(ty: A2lType, raw: &[u8], byte_order: super::ByteOrder) -> A2lValue {
    let sz = ty.size();
    if sz == 0 || raw.len() < sz {
        return A2lValue::Raw(raw.to_vec());
    }
    let le = matches!(byte_order, super::ByteOrder::LittleEndian);
    let read_u = |n: usize| -> u64 {
        let mut v: u64 = 0;
        if le {
            for &byte in raw[..n].iter().rev() {
                v = (v << 8) | byte as u64;
            }
        } else {
            for &byte in raw[..n].iter() {
                v = (v << 8) | byte as u64;
            }
        }
        v
    };
    match ty {
        A2lType::UByte => A2lValue::Unsigned(raw[0] as u64),
        A2lType::SByte => A2lValue::Signed(raw[0] as i8 as i64),
        A2lType::UWord => A2lValue::Unsigned(read_u(2)),
        A2lType::SWord => A2lValue::Signed(read_u(2) as u16 as i16 as i64),
        A2lType::ULong => A2lValue::Unsigned(read_u(4)),
        A2lType::SLong => A2lValue::Signed(read_u(4) as u32 as i32 as i64),
        A2lType::AUint64 => A2lValue::Unsigned(read_u(8)),
        A2lType::AInt64 => A2lValue::Signed(read_u(8) as i64),
        A2lType::Float32 => {
            let bits = read_u(4) as u32;
            A2lValue::Float(f32::from_bits(bits) as f64)
        }
        A2lType::Float64 => {
            let bits = read_u(8);
            A2lValue::Float(f64::from_bits(bits))
        }
        A2lType::Unknown => A2lValue::Raw(raw.to_vec()),
    }
}

// ─── Tokenizer & block parsers ──────────────────────────────────────────────

/// Split A2L source into whitespace-delimited tokens, stripping comments and
/// keeping quoted strings as single (unquoted) tokens.
fn tokenize(src: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    let n = bytes.len();
    while i < n {
        let c = bytes[i] as char;
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && i + 1 < n && bytes[i + 1] == b'*' {
            // Block comment.
            i += 2;
            while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else if c == '/' && i + 1 < n && bytes[i + 1] == b'/' {
            // Line comment (but not /begin or /end — those start with '/' + letter).
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
        } else if c == '"' {
            // Quoted string.
            i += 1;
            let start = i;
            while i < n && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < n {
                    i += 1;
                }
                i += 1;
            }
            tokens.push(src[start..i].to_string());
            i += 1; // skip closing quote
        } else {
            let start = i;
            while i < n {
                let ch = bytes[i] as char;
                if ch.is_whitespace() || ch == '"' {
                    break;
                }
                i += 1;
            }
            tokens.push(src[start..i].to_string());
        }
    }
    tokens
}

/// Parse a hex (`0x…`) or decimal integer token into a `u32` address.
fn parse_address(tok: &str) -> Option<u32> {
    let t = tok.trim();
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).ok()
    } else {
        t.parse::<u32>().ok()
    }
}

/// Find the index of the matching `/end <keyword>` starting from `from`.
fn find_end(tokens: &[String], from: usize, keyword: &str) -> Option<usize> {
    let mut i = from;
    while i < tokens.len() {
        if tokens[i] == "/end" && i + 1 < tokens.len() && tokens[i + 1] == keyword {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Parse a MEASUREMENT block. `start` points just past `MEASUREMENT`.
/// Positional layout: Name, LongIdentifier, Datatype, Conversion, Resolution,
/// Accuracy, LowerLimit, UpperLimit; ECU_ADDRESS is an optional keyword.
fn parse_measurement(tokens: &[String], start: usize) -> Option<(A2lObject, usize)> {
    let end = find_end(tokens, start, "MEASUREMENT")?;
    let body = &tokens[start..end];
    let name = body.first()?.clone();
    let datatype = body
        .get(2)
        .map(|s| A2lType::parse(s))
        .unwrap_or(A2lType::Unknown);
    let mut address = None;
    let mut k = 0;
    while k < body.len() {
        if body[k] == "ECU_ADDRESS" {
            if let Some(a) = body.get(k + 1).and_then(|s| parse_address(s)) {
                address = Some(a);
            }
            break;
        }
        k += 1;
    }
    let address = address?;
    Some((
        A2lObject {
            name,
            address,
            datatype,
            is_characteristic: false,
        },
        end + 2,
    ))
}

/// Parse a CHARACTERISTIC block. `start` points just past `CHARACTERISTIC`.
/// Positional layout: Name, LongIdentifier, Type, Address, Deposit, MaxDiff,
/// Conversion, LowerLimit, UpperLimit. Datatype is deferred to RECORD_LAYOUT
/// and left Unknown.
fn parse_characteristic(tokens: &[String], start: usize) -> Option<(A2lObject, usize)> {
    let end = find_end(tokens, start, "CHARACTERISTIC")?;
    let body = &tokens[start..end];
    let name = body.first()?.clone();
    let address = parse_address(body.get(3)?)?;
    Some((
        A2lObject {
            name,
            address,
            datatype: A2lType::Unknown,
            is_characteristic: true,
        },
        end + 2,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xcp::ByteOrder;

    const SAMPLE: &str = r#"
        /* header comment */
        ASAP2_VERSION 1 60
        /begin MEASUREMENT engine_speed "Engine speed"
            UWORD NO_COMPU_METHOD 0 0 0 65535
            ECU_ADDRESS 0x20000000
        /end MEASUREMENT

        /begin MEASUREMENT coolant_temp "Coolant temperature"
            SBYTE NO_COMPU_METHOD 0 0 -40 120
            ECU_ADDRESS 0x20000002
        /end MEASUREMENT

        /begin CHARACTERISTIC max_rpm "Rev limiter"
            VALUE 0x20001000 __UBYTE_Z 0 NO_COMPU_METHOD 0 8000
        /end CHARACTERISTIC
    "#;

    #[test]
    fn parses_measurements_and_characteristic() {
        let db = A2lDatabase::parse(SAMPLE);
        assert_eq!(db.len(), 3);
        assert_eq!(db.name_for_address(0x2000_0000), Some("engine_speed"));
        assert_eq!(db.name_for_address(0x2000_0002), Some("coolant_temp"));
        assert_eq!(db.name_for_address(0x2000_1000), Some("max_rpm"));
    }

    #[test]
    fn measurement_datatype_recorded() {
        let db = A2lDatabase::parse(SAMPLE);
        assert_eq!(db.object_at(0x2000_0000).unwrap().datatype, A2lType::UWord);
        assert_eq!(db.object_at(0x2000_0002).unwrap().datatype, A2lType::SByte);
        assert!(db.object_at(0x2000_1000).unwrap().is_characteristic);
    }

    #[test]
    fn decode_uword_little_endian() {
        let db = A2lDatabase::parse(SAMPLE);
        assert_eq!(
            db.decode_at(0x2000_0000, &[0x10, 0x27], ByteOrder::LittleEndian),
            Some(A2lValue::Unsigned(10000))
        );
    }

    #[test]
    fn decode_sbyte_negative() {
        let db = A2lDatabase::parse(SAMPLE);
        assert_eq!(
            db.decode_at(0x2000_0002, &[0xD8], ByteOrder::LittleEndian),
            Some(A2lValue::Signed(-40))
        );
    }

    #[test]
    fn decode_float32_big_endian() {
        // 1.0f32 big-endian = 3F 80 00 00
        assert_eq!(
            decode_value(
                A2lType::Float32,
                &[0x3F, 0x80, 0x00, 0x00],
                ByteOrder::BigEndian
            ),
            A2lValue::Float(1.0)
        );
    }

    #[test]
    fn unknown_address_is_none() {
        let db = A2lDatabase::parse(SAMPLE);
        assert!(db
            .decode_at(0xDEAD, &[0x00], ByteOrder::LittleEndian)
            .is_none());
    }

    #[test]
    fn empty_input_is_empty_db() {
        let db = A2lDatabase::parse("");
        assert!(db.is_empty());
    }
}
