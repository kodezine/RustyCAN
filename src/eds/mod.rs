pub mod types;

use std::collections::HashMap;
use std::io::{self, BufRead};
use std::path::Path;

use types::{AccessType, DataType, ObjectDictionary, OdEntry};

/// Parse a CANopen EDS file and return an `ObjectDictionary`.
///
/// EDS is an INI-format file where section names are hex indices or
/// `<index>sub<sub>`. For example:
///   `[1000]`     → object 0x1000, subindex 0
///   `[1A00sub1]` → object 0x1A00, subindex 1
pub fn parse_eds<P: AsRef<Path>>(path: P) -> Result<ObjectDictionary, io::Error> {
    let file = std::fs::File::open(path)?;
    let reader = io::BufReader::new(file);

    let mut od = ObjectDictionary::new();
    let mut current_key: Option<(u16, u8)> = None;
    let mut fields: HashMap<String, String> = HashMap::new();

    for line in reader.lines() {
        let line = line?;
        let line = line.trim();

        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            // Flush previous section.
            if let Some(key) = current_key.take() {
                if let Some(entry) = build_entry(&fields) {
                    od.insert(key, entry);
                }
            }
            fields.clear();
            current_key = parse_section(&line[1..line.len() - 1]);
        } else if let Some(eq) = line.find('=') {
            let k = line[..eq].trim().to_lowercase();
            let v = line[eq + 1..].trim().to_string();
            fields.insert(k, v);
        }
    }

    // Flush final section.
    if let Some(key) = current_key.take() {
        if let Some(entry) = build_entry(&fields) {
            od.insert(key, entry);
        }
    }

    Ok(od)
}

/// Parse a section name like "1000", "1a00sub1", "1A00Sub2".
/// Returns `None` for non-object sections (e.g. "FileInfo", "DeviceInfo").
fn parse_section(name: &str) -> Option<(u16, u8)> {
    let upper = name.to_uppercase();
    if let Some(sub_pos) = upper.find("SUB") {
        let hex_idx = upper[..sub_pos].trim();
        let hex_sub = upper[sub_pos + 3..].trim();
        let index = u16::from_str_radix(hex_idx, 16).ok()?;
        let subindex = u8::from_str_radix(hex_sub, 16).ok()?;
        Some((index, subindex))
    } else {
        // Only treat as object section if it is a pure hex number 1–4 digits.
        let trimmed = upper.trim();
        if !trimmed.is_empty()
            && trimmed.len() <= 4
            && trimmed.chars().all(|c| c.is_ascii_hexdigit())
        {
            let index = u16::from_str_radix(trimmed, 16).ok()?;
            Some((index, 0))
        } else {
            None
        }
    }
}

/// Build an `OdEntry` from the INI key-value fields of one section.
/// Returns `None` if the section lacks a `ParameterName`.
fn build_entry(fields: &HashMap<String, String>) -> Option<OdEntry> {
    let name = fields.get("parametername")?.clone();
    let data_type = fields
        .get("datatype")
        .and_then(|v| parse_u16(v))
        .map(DataType::from_code)
        .unwrap_or(DataType::Unknown(0));
    let access = fields
        .get("accesstype")
        .map(|v| AccessType::parse(v))
        .unwrap_or(AccessType::Unknown);
    let default_value = fields.get("defaultvalue").cloned();

    Some(OdEntry {
        name,
        data_type,
        access,
        default_value,
    })
}

/// Parse a value string that may be `0x...` hex or plain decimal.
fn parse_u16(s: &str) -> Option<u16> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16).ok()
    } else {
        s.parse().ok()
    }
}

/// Parse a `DefaultValue` string as `u32` (hex or decimal).
pub fn parse_default_u32(s: &str) -> Option<u32> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).ok()
    } else {
        s.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_section_var() {
        assert_eq!(parse_section("1000"), Some((0x1000, 0)));
        assert_eq!(parse_section("1A00"), Some((0x1A00, 0)));
    }

    #[test]
    fn parse_section_sub() {
        assert_eq!(parse_section("1A00sub1"), Some((0x1A00, 1)));
        assert_eq!(parse_section("1A00Sub2"), Some((0x1A00, 2)));
        assert_eq!(parse_section("1a00SUB0A"), Some((0x1A00, 0x0A)));
    }

    #[test]
    fn parse_section_non_object() {
        assert_eq!(parse_section("FileInfo"), None);
        assert_eq!(parse_section("DeviceInfo"), None);
        assert_eq!(parse_section("Comments"), None);
    }

    #[test]
    fn parse_default_u32_hex() {
        assert_eq!(parse_default_u32("0x0000C350"), Some(50000));
        assert_eq!(parse_default_u32("0xFF"), Some(255));
    }

    #[test]
    fn parse_default_u32_decimal() {
        assert_eq!(parse_default_u32("1234"), Some(1234));
    }

    #[test]
    fn build_entry_minimal() {
        let mut fields = HashMap::new();
        fields.insert("parametername".into(), "DeviceType".into());
        fields.insert("datatype".into(), "0x0007".into());
        fields.insert("accesstype".into(), "ro".into());
        let entry = build_entry(&fields).expect("should build");
        assert_eq!(entry.name, "DeviceType");
        assert_eq!(entry.data_type, DataType::Unsigned32);
        assert_eq!(entry.access, AccessType::ReadOnly);
    }
}
