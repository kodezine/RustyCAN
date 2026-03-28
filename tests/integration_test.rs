//! Integration tests for EDS parsing and PDO decoding using a real fixture file.

use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

// ─── EDS parser ──────────────────────────────────────────────────────────────

#[test]
fn eds_parse_device_type() {
    let od = rustycan::eds::parse_eds(fixture("sample_drive.eds"))
        .expect("failed to parse EDS");

    let entry = od.get(&(0x1000, 0)).expect("0x1000/0 missing");
    assert_eq!(entry.name, "Device Type");
    assert_eq!(entry.data_type, rustycan::eds::types::DataType::Unsigned32);
    assert_eq!(entry.access, rustycan::eds::types::AccessType::ReadOnly);
}

#[test]
fn eds_parse_controlword() {
    let od = rustycan::eds::parse_eds(fixture("sample_drive.eds"))
        .expect("failed to parse EDS");

    let entry = od.get(&(0x6040, 0)).expect("0x6040/0 missing");
    assert_eq!(entry.name, "ControlWord");
    assert_eq!(entry.data_type, rustycan::eds::types::DataType::Unsigned16);
    assert_eq!(entry.access, rustycan::eds::types::AccessType::ReadWrite);
}

#[test]
fn eds_parse_tpdo1_sub_objects() {
    let od = rustycan::eds::parse_eds(fixture("sample_drive.eds"))
        .expect("failed to parse EDS");

    // Comm params entry for TPDO1
    let cob_entry = od.get(&(0x1800, 1)).expect("0x1800/1 missing");
    assert_eq!(cob_entry.name, "COB-ID use by TPDO 1");
    assert_eq!(
        cob_entry.default_value.as_deref(),
        Some("0x00000181")
    );

    // Mapping entries
    let num = od.get(&(0x1A00, 0)).expect("0x1A00/0 missing");
    assert_eq!(num.default_value.as_deref(), Some("2"));

    let map1 = od.get(&(0x1A00, 1)).expect("0x1A00/1 missing");
    assert_eq!(map1.default_value.as_deref(), Some("0x60410010"));

    let map2 = od.get(&(0x1A00, 2)).expect("0x1A00/2 missing");
    assert_eq!(map2.default_value.as_deref(), Some("0x60440010"));
}

// ─── PDO decoder ─────────────────────────────────────────────────────────────

#[test]
fn pdo_decoder_builds_from_eds() {
    use rustycan::canopen::pdo::{PdoDecoder, PdoRawValue};

    let od = rustycan::eds::parse_eds(fixture("sample_drive.eds"))
        .expect("failed to parse EDS");

    let decoder = PdoDecoder::from_od(1, &od);

    // TPDO1 COB-ID = 0x181 (node 1)
    assert!(
        decoder.mappings.contains_key(&0x181),
        "TPDO1 (0x181) not found in decoder"
    );

    // Decode a known payload:
    //   StatusWord  (0x6041) = 0x0027  → [0x27, 0x00]
    //   VelocityActualValue (0x6044) = 0x1234 → [0x34, 0x12]
    let payload: [u8; 8] = [0x27, 0x00, 0x34, 0x12, 0x00, 0x00, 0x00, 0x00];
    let values = decoder
        .decode(0x181, &payload)
        .expect("decode returned None");

    assert_eq!(values.len(), 2);
    assert_eq!(values[0].signal_name, "StatusWord");
    assert!(matches!(values[0].value, PdoRawValue::Unsigned(0x0027)));

    assert_eq!(values[1].signal_name, "VelocityActualValue");
    assert!(matches!(values[1].value, PdoRawValue::Unsigned(0x1234)));
}

// ─── SDO decode with EDS lookup ───────────────────────────────────────────────

#[test]
fn sdo_decodes_controlword_with_eds() {
    use rustycan::canopen::sdo::{SdoValue, decode_sdo};

    let od = rustycan::eds::parse_eds(fixture("sample_drive.eds"))
        .expect("failed to parse EDS");

    // Expedited upload response: server sends ControlWord = 0x000F
    // cs=0x4B (2 bytes), index=0x6040 LE, subindex=0, data=[0x0F, 0x00, ...]
    let frame: [u8; 8] = [0x4B, 0x40, 0x60, 0x00, 0x0F, 0x00, 0x00, 0x00];
    let ev = decode_sdo(1, &frame, &od, true).expect("decode_sdo returned None");

    assert_eq!(ev.name, "ControlWord");
    assert_eq!(ev.node_id, 1);
    assert!(matches!(ev.value, Some(SdoValue::U16(0x000F))));
}

// ─── NMT frame classification ─────────────────────────────────────────────────

#[test]
fn classify_heartbeat_frame() {
    use rustycan::canopen::{FrameType, classify_frame};

    assert_eq!(classify_frame(0x701), FrameType::Heartbeat(1));
    assert_eq!(classify_frame(0x702), FrameType::Heartbeat(2));
}

#[test]
fn classify_tpdo_rpdo() {
    use rustycan::canopen::{FrameType, classify_frame};

    assert_eq!(classify_frame(0x181), FrameType::Tpdo(1, 1));
    assert_eq!(classify_frame(0x201), FrameType::Rpdo(1, 1));
    assert_eq!(classify_frame(0x281), FrameType::Tpdo(2, 1));
}
