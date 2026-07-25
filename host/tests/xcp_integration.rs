//! XCP-on-CAN end-to-end fixture test.
//!
//! Replays a recorded XCP-on-CAN session (a deterministic byte-level fixture)
//! through the public `rustycan::xcp` decode stack — transport classification,
//! command/response codec, dynamic DAQ tracking, and A2L name resolution — and
//! asserts the decoded results. No hardware or adapter is involved.
//!
//! This stands in for a live XCP slave until the firmware team adds an XCP stack
//! to one of the dongle firmwares; the same decode path runs against real bus
//! traffic in `host/src/session.rs`.

use rustycan::xcp::a2l::{A2lDatabase, A2lValue};
use rustycan::xcp::command::{self, XcpCommand};
use rustycan::xcp::daq::DaqTracker;
use rustycan::xcp::{classify, ByteOrder, XcpConfig, XcpFrameType};

const CRO: u32 = 0x600;
const DTO: u32 = 0x601;

/// One recorded frame: `(can_id, payload)`.
type Frame = (u32, &'static [u8]);

/// A minimal recorded XCP session: connect, configure a DAQ list, and stream one
/// measurement frame. Little-endian slave, MAX_CTO = 8.
const SESSION: &[Frame] = &[
    // 1. CONNECT command (master → slave).
    (CRO, &[0xFF, 0x00]),
    // 2. CONNECT positive response: LE, byte AG, max_cto=8, max_dto=8, v1/v1.
    (DTO, &[0xFF, 0x15, 0x00, 0x08, 0x08, 0x00, 0x01, 0x01]),
    // 3. Dynamic DAQ config: point at DAQ 0 / ODT 0 / entry 0.
    (CRO, &[0xE2, 0x00, 0x00, 0x00, 0x00, 0x00]),
    // 4. WRITE_DAQ: 2-byte element at 0x20000000.
    (CRO, &[0xE1, 0xFF, 0x02, 0x00, 0x00, 0x00, 0x00, 0x20]),
    // 5. WRITE_DAQ: 2-byte element at 0x20000002.
    (CRO, &[0xE1, 0xFF, 0x02, 0x00, 0x02, 0x00, 0x00, 0x20]),
    // 6. START_STOP_DAQ_LIST (select) for DAQ 0.
    (CRO, &[0xDE, 0x02, 0x00, 0x00]),
    // 7. Positive response carrying first_pid = 0x10.
    (DTO, &[0xFF, 0x10]),
    // 8. DAQ measurement frame: PID 0x10, then two u16 values (LE).
    (DTO, &[0x10, 0x10, 0x27, 0xD0, 0x07]),
];

/// A tiny A2L describing the two measured addresses.
const A2L: &str = r#"
    /begin MEASUREMENT engine_speed "rpm"
        UWORD NO_COMPU_METHOD 0 0 0 65535
        ECU_ADDRESS 0x20000000
    /end MEASUREMENT
    /begin MEASUREMENT boost_pressure "kPa"
        UWORD NO_COMPU_METHOD 0 0 0 65535
        ECU_ADDRESS 0x20000002
    /end MEASUREMENT
"#;

#[test]
fn replays_full_xcp_session() {
    let cfg = XcpConfig {
        cro_id: CRO,
        dto_id: DTO,
    };
    let a2l = A2lDatabase::parse(A2L);
    let mut byte_order = ByteOrder::default();
    let mut daq = DaqTracker::new();
    let mut daq_start_pending: Option<u16> = None;
    let mut connected = false;
    let mut decoded_samples: Vec<(u32, A2lValue)> = Vec::new();

    for &(can_id, data) in SESSION {
        let ft = classify(&cfg, can_id, data).expect("fixture frames are all XCP");
        match ft {
            XcpFrameType::Command => {
                let cmd = command::decode_command(data, byte_order).unwrap();
                if let Some(daq_num) = daq.on_command(&cmd) {
                    daq_start_pending = Some(daq_num);
                }
            }
            XcpFrameType::Response => {
                if let Some(daq_num) = daq_start_pending.take() {
                    let first_pid = data.get(1).copied().unwrap_or(0);
                    daq.on_start_pid(daq_num, first_pid);
                } else if let Some(cr) = command::decode_connect_response(data) {
                    byte_order = cr.byte_order;
                    connected = true;
                    assert_eq!(cr.max_cto, 8);
                }
            }
            XcpFrameType::Daq(_) => {
                for s in daq.decode(data).unwrap() {
                    let value = a2l
                        .decode_at(s.address, &s.raw, byte_order)
                        .expect("address is described by the A2L");
                    decoded_samples.push((s.address, value));
                }
            }
            other => panic!("unexpected frame type in fixture: {other:?}"),
        }
    }

    // CONNECT was acknowledged and byte order negotiated.
    assert!(
        connected,
        "CONNECT response should mark the session connected"
    );
    assert_eq!(byte_order, ByteOrder::LittleEndian);

    // The DAQ frame decoded both measured elements, in order.
    assert_eq!(decoded_samples.len(), 2);
    assert_eq!(
        decoded_samples[0],
        (0x2000_0000, A2lValue::Unsigned(0x2710)) // 10000
    );
    assert_eq!(
        decoded_samples[1],
        (0x2000_0002, A2lValue::Unsigned(0x07D0)) // 2000
    );

    // A2L resolved both addresses to their measurement names.
    assert_eq!(a2l.name_for_address(0x2000_0000), Some("engine_speed"));
    assert_eq!(a2l.name_for_address(0x2000_0002), Some("boost_pressure"));
}

#[test]
fn passive_command_decode_matches_fixture() {
    // Sanity: the recorded CRO frames decode to the expected command variants.
    assert_eq!(
        command::decode_command(&[0xFF, 0x00], ByteOrder::LittleEndian),
        Some(XcpCommand::Connect { mode: 0 })
    );
    assert_eq!(
        command::decode_command(
            &[0xE1, 0xFF, 0x02, 0x00, 0x00, 0x00, 0x00, 0x20],
            ByteOrder::LittleEndian
        ),
        Some(XcpCommand::WriteDaq {
            bit_offset: 0xFF,
            size: 2,
            addr_ext: 0,
            address: 0x2000_0000,
        })
    );
}

#[test]
fn xcp_ids_do_not_shadow_other_traffic() {
    let cfg = XcpConfig {
        cro_id: CRO,
        dto_id: DTO,
    };
    // A CANopen TPDO1 on 0x181 is not XCP and must fall through.
    assert!(classify(&cfg, 0x181, &[0x00]).is_none());
    // The configured CRO/DTO are XCP.
    assert!(classify(&cfg, CRO, &[0xFF]).is_some());
    assert!(classify(&cfg, DTO, &[0xFF]).is_some());
}
