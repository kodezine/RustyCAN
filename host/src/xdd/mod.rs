//! CiA 311 XDD (XML Device Description) file parser.
//!
//! Parses the XML-based `xdd` / `xdd2` format used by CANopen FD devices and
//! emits the same [`ObjectDictionary`] structure produced by the EDS INI parser,
//! so all downstream code (PDO decoders, SDO browser, etc.) works unchanged.
//!
//! # Supported XDD constructs
//!
//! - `<CANopenObject>` with attributes: `index`, `name`, `objectType`, `dataType`,
//!   `defaultValue`, `PDOmapping`, `accessType`
//! - `<CANopenSubObject>` with attributes: `subIndex`, `name`, `dataType`,
//!   `defaultValue`, `accessType`
//! - `nrOfEntries` sub-object (subIndex=0) for determining extended PDO counts

use std::io;
use std::path::Path;

use crate::eds::types::{AccessType, DataType, ObjectDictionary, OdEntry};

/// Parse a CANopen XDD file and return an [`ObjectDictionary`].
///
/// Returns an error if the file cannot be read or contains malformed XML.
/// Individual objects that cannot be parsed are silently skipped.
pub fn parse_xdd<P: AsRef<Path>>(path: P) -> Result<ObjectDictionary, io::Error> {
    let xml = std::fs::read_to_string(path).map_err(|e| io::Error::new(e.kind(), e.to_string()))?;

    let doc = roxmltree::Document::parse(&xml)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    let mut od = ObjectDictionary::new();

    // Walk all <CANopenObject> elements anywhere in the document.
    for node in doc
        .descendants()
        .filter(|n| n.has_tag_name("CANopenObject"))
    {
        let Some(index) = node.attribute("index").and_then(parse_hex_u16) else {
            continue;
        };

        // Attributes from the object element itself (used as defaults for VAR objects
        // that have no sub-objects).
        let obj_name = node.attribute("name").unwrap_or("").to_string();
        let obj_dtype = node
            .attribute("dataType")
            .and_then(parse_hex_u16)
            .map(DataType::from_code)
            .unwrap_or(DataType::Unknown(0));
        let obj_access = node
            .attribute("accessType")
            .map(AccessType::parse)
            .unwrap_or(AccessType::Unknown);
        let obj_default = node.attribute("defaultValue").map(|s| s.to_string());

        // Collect <CANopenSubObject> children.
        let sub_nodes: Vec<_> = node
            .children()
            .filter(|n| n.has_tag_name("CANopenSubObject"))
            .collect();

        if sub_nodes.is_empty() {
            // VAR object — insert at subindex 0.
            od.insert(
                (index, 0),
                OdEntry {
                    name: obj_name,
                    data_type: obj_dtype,
                    access: obj_access,
                    default_value: obj_default,
                },
            );
        } else {
            for sub in sub_nodes {
                let Some(subindex) = sub.attribute("subIndex").and_then(parse_hex_u8) else {
                    continue;
                };

                let name = sub.attribute("name").unwrap_or(&obj_name).to_string();
                let dtype = sub
                    .attribute("dataType")
                    .and_then(parse_hex_u16)
                    .map(DataType::from_code)
                    .unwrap_or(DataType::Unknown(0));
                let access = sub
                    .attribute("accessType")
                    .map(AccessType::parse)
                    .unwrap_or(AccessType::Unknown);
                let default_value = sub.attribute("defaultValue").map(|s| s.to_string());

                od.insert(
                    (index, subindex),
                    OdEntry {
                        name,
                        data_type: dtype,
                        access,
                        default_value,
                    },
                );
            }
        }
    }

    Ok(od)
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Parse a hex string (with or without `0x` prefix) into a `u16`.
fn parse_hex_u16(s: &str) -> Option<u16> {
    let s = s.trim();
    let hex = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    u16::from_str_radix(hex, 16).ok()
}

/// Parse a hex string (with or without `0x` prefix) into a `u8`.
fn parse_hex_u8(s: &str) -> Option<u8> {
    let s = s.trim();
    let hex = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    u8::from_str_radix(hex, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_xdd() {
        let xml = r#"<?xml version="1.0"?>
<ISO15745ProfileContainer>
  <ProfileBody>
    <ApplicationLayers>
      <CANopenObjectList>
        <CANopenObject index="0x1000" name="Device Type" objectType="0x07" dataType="0x0007" defaultValue="0x00000000" accessType="ro"/>
        <CANopenObject index="0x1017" name="Producer Heartbeat Time" objectType="0x07" dataType="0x0006" defaultValue="0" accessType="rw"/>
        <CANopenObject index="0x1A00" name="TPDO1 Mapping" objectType="0x09" dataType="0x0005">
          <CANopenSubObject subIndex="0x00" name="Number of mapped objects" dataType="0x0005" defaultValue="2" accessType="rw"/>
          <CANopenSubObject subIndex="0x01" name="Signal A" dataType="0x0007" defaultValue="0x62000108" accessType="rw"/>
          <CANopenSubObject subIndex="0x02" name="Signal B" dataType="0x0006" defaultValue="0x62000210" accessType="rw"/>
        </CANopenObject>
      </CANopenObjectList>
    </ApplicationLayers>
  </ProfileBody>
</ISO15745ProfileContainer>"#;

        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), xml).unwrap();
        let od = parse_xdd(tmp.path()).unwrap();

        // VAR object at (0x1000, 0)
        let entry = od.get(&(0x1000, 0)).unwrap();
        assert_eq!(entry.name, "Device Type");
        assert_eq!(entry.data_type, DataType::Unsigned32);
        assert_eq!(entry.default_value.as_deref(), Some("0x00000000"));

        // Sub-object at (0x1A00, 0)
        let sub0 = od.get(&(0x1A00, 0)).unwrap();
        assert_eq!(sub0.name, "Number of mapped objects");
        assert_eq!(sub0.default_value.as_deref(), Some("2"));

        // Sub-object at (0x1A00, 1)
        assert!(od.contains_key(&(0x1A00, 1)));
    }
}
