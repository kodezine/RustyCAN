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
    let od = rustycan::eds::parse_eds(fixture("sample_drive.eds")).expect("failed to parse EDS");

    let entry = od.get(&(0x1000, 0)).expect("0x1000/0 missing");
    assert_eq!(entry.name, "Device Type");
    assert_eq!(entry.data_type, rustycan::eds::types::DataType::Unsigned32);
    assert_eq!(entry.access, rustycan::eds::types::AccessType::ReadOnly);
}

#[test]
fn eds_parse_status_word() {
    let od = rustycan::eds::parse_eds(fixture("sample_drive.eds")).expect("failed to parse EDS");

    // Status Container (0x3000) sub1 = "Status Word", Unsigned16, read-only
    let entry = od.get(&(0x3000, 1)).expect("0x3000/1 missing");
    assert_eq!(entry.name, "Status Word");
    assert_eq!(entry.data_type, rustycan::eds::types::DataType::Unsigned16);
    assert_eq!(entry.access, rustycan::eds::types::AccessType::ReadOnly);
}

#[test]
fn eds_parse_tpdo1_sub_objects() {
    let od = rustycan::eds::parse_eds(fixture("sample_drive.eds")).expect("failed to parse EDS");

    // Comm params entry for TPDO1
    let cob_entry = od.get(&(0x1800, 1)).expect("0x1800/1 missing");
    assert_eq!(cob_entry.name, "COB-ID used by TPDO");
    assert_eq!(cob_entry.default_value.as_deref(), Some("0x201"));

    // Mapping count = 3 (Status Bits, Digital Sensors, Current Segment)
    let num = od.get(&(0x1A00, 0)).expect("0x1A00/0 missing");
    assert_eq!(num.default_value.as_deref(), Some("3"));

    let map1 = od.get(&(0x1A00, 1)).expect("0x1A00/1 missing");
    assert_eq!(map1.default_value.as_deref(), Some("0x30000110")); // 0x3000/1, 16-bit

    let map2 = od.get(&(0x1A00, 2)).expect("0x1A00/2 missing");
    assert_eq!(map2.default_value.as_deref(), Some("0x30000208")); // 0x3000/2, 8-bit
}

// ─── PDO decoder ─────────────────────────────────────────────────────────────

#[test]
fn pdo_decoder_builds_from_eds() {
    use rustycan::canopen::pdo::{PdoDecoder, PdoRawValue};

    let od = rustycan::eds::parse_eds(fixture("sample_drive.eds")).expect("failed to parse EDS");

    let decoder = PdoDecoder::from_od(32, &od);

    // TPDO1 COB-ID = 0x201 (as configured in EDS [1800sub1])
    assert!(
        decoder.mappings.contains_key(&0x201),
        "TPDO1 (0x201) not found in decoder"
    );

    // Decode a known payload (4 bytes: 16-bit Status Bits + 8-bit Digital Inputs + 8-bit Segment):
    //   DB Status Bits     (0x3000/1) = 0x0027  → [0x27, 0x00]
    //   Digital Inputs     (0x3000/2) = 0xAB    → [0xAB]
    //   Current Segment    (0x3000/5) = 0x03    → [0x03]
    let payload: [u8; 8] = [0x27, 0x00, 0xAB, 0x03, 0x00, 0x00, 0x00, 0x00];
    let values = decoder
        .decode(0x201, &payload)
        .expect("decode returned None");

    assert_eq!(values.len(), 3);
    assert_eq!(values[0].signal_name, "Status Word");
    assert!(matches!(values[0].value, PdoRawValue::Unsigned(0x0027)));

    assert_eq!(values[1].signal_name, "Digital Inputs");
    assert!(matches!(values[1].value, PdoRawValue::Unsigned(0xAB)));

    assert_eq!(values[2].signal_name, "Current Segment Index");
    assert!(matches!(values[2].value, PdoRawValue::Unsigned(0x03)));
}

// ─── SDO decode with EDS lookup ───────────────────────────────────────────────

#[test]
fn sdo_decodes_status_word_with_eds() {
    use rustycan::canopen::sdo::{decode_sdo, SdoValue};

    let od = rustycan::eds::parse_eds(fixture("sample_drive.eds")).expect("failed to parse EDS");

    // Expedited upload response: server sends Status Word (0x3000/1) = 0x00FF
    // cs=0x4B (2-byte expedited), index=0x3000 LE, subindex=1, data=[0xFF, 0x00, ...]
    let frame: [u8; 8] = [0x4B, 0x00, 0x30, 0x01, 0xFF, 0x00, 0x00, 0x00];
    let ev = decode_sdo(32, &frame, &od, true).expect("decode_sdo returned None");

    assert_eq!(ev.name, "Status Word");
    assert_eq!(ev.node_id, 32);
    assert!(matches!(ev.value, Some(SdoValue::U16(0x00FF))));
}

// ─── SDO frame encoding ───────────────────────────────────────────────────────

#[test]
fn sdo_encode_upload_request_roundtrip() {
    use rustycan::canopen::sdo::{decode_sdo, encode_upload_request};
    use rustycan::eds::types::ObjectDictionary;

    let frame = encode_upload_request(0x3000, 1);
    // COB-ID for request is 0x600+n, but decode_sdo is direction-agnostic for cs=0x40
    let od = ObjectDictionary::new();
    let ev = decode_sdo(32, &frame, &od, false).expect("encode_upload_request produced bad frame");
    assert_eq!(ev.index, 0x3000);
    assert_eq!(ev.subindex, 0x01);
}

#[test]
fn sdo_encode_download_expedited_u16() {
    use rustycan::canopen::sdo::{decode_sdo, encode_download_expedited};

    let od = rustycan::eds::parse_eds(fixture("sample_drive.eds")).expect("failed to parse EDS");

    // Write 0x000F to Status Word (0x3000/1, U16)
    let data = 0x000Fu16.to_le_bytes();
    let frame = encode_download_expedited(0x3000, 1, &data).expect("encode failed");

    // Decoding as a client→server request
    let ev = decode_sdo(32, &frame, &od, false).expect("decode returned None");
    assert_eq!(ev.index, 0x3000);
    assert_eq!(ev.subindex, 0x01);
}

#[test]
fn sdo_encode_value_u32_known_object() {
    use rustycan::canopen::sdo::{encode_download_expedited, encode_value_for_type};
    use rustycan::eds::types::DataType;

    // Encode 0xDEAD as U32 → 4 LE bytes that fit in an expedited frame
    let bytes = encode_value_for_type("0xDEAD", &DataType::Unsigned32).unwrap();
    let frame = encode_download_expedited(0x1000, 0, &bytes).expect("encode failed");
    // cs must be 0x23 (0 bytes unused in data)
    assert_eq!(frame[0], 0x23);
    assert_eq!(u32::from_le_bytes(frame[4..8].try_into().unwrap()), 0xDEAD);
}

#[test]
fn sdo_parse_hex_bytes_and_segmented_init() {
    use rustycan::canopen::sdo::{decode_segmented_upload_initiate, parse_hex_bytes};

    let bytes = parse_hex_bytes("48 65 6C 6C 6F").unwrap();
    assert_eq!(bytes, b"Hello");

    // Build a segmented upload initiate response with size=5
    let mut frame = [0u8; 8];
    frame[0] = 0x41; // CS: not expedited, size indicated
    frame[4..8].copy_from_slice(&5u32.to_le_bytes());
    assert_eq!(decode_segmented_upload_initiate(&frame), Some(Some(5)));
}

// ─── NMT frame classification ─────────────────────────────────────────────────

#[test]
fn classify_heartbeat_frame() {
    use rustycan::canopen::{classify_frame, FrameType};

    assert_eq!(classify_frame(0x701), FrameType::Heartbeat(1));
    assert_eq!(classify_frame(0x702), FrameType::Heartbeat(2));
}

#[test]
fn classify_tpdo_rpdo() {
    use rustycan::canopen::{classify_frame, FrameType};

    assert_eq!(classify_frame(0x181), FrameType::Tpdo(1, 1));
    assert_eq!(classify_frame(0x201), FrameType::Rpdo(1, 1));
    assert_eq!(classify_frame(0x281), FrameType::Tpdo(2, 1));
}
