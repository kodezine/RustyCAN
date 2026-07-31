//! Apex USB-CAN wire protocol — clean-room encode/decode.
//!
//! This module is a from-scratch MIT implementation of the observable USB wire
//! format of the Apex USB-CAN (running/application mode).  It was
//! derived from bus captures of the device (issue #103, Phase 0b), not from any
//! GPL driver source, and operates on plain integers/bytes so it is fully
//! unit-testable without hardware.
//!
//! # Running-mode USB interface (PID 0x1101 / 0x1181 / 0x1122)
//!
//! Five endpoints:
//! * `0x01` bulk OUT — CAN frames to send (DATA_OUT)
//! * `0x02` bulk OUT — 8-byte command messages (MSG_OUT)
//! * `0x81` bulk IN  — received CAN frames (DATA_IN)
//! * `0x82` bulk IN  — 8-byte command replies (MSG_IN)
//! * `0x83` interrupt IN — 4-byte status words (STAT_IN)
//!
//! Commands are 8 bytes `[opcode][args…][chan@6]`; the device echoes the same
//! opcode with bit 0x80 set.  CAN frames are a fixed 16-byte record.

#![allow(dead_code)] // Phase 2 (adapter open/recv/send) consumes these; wired next.

// ─── Endpoints ────────────────────────────────────────────────────────────────

pub const EP_DATA_OUT: u8 = 0x01;
pub const EP_MSG_OUT: u8 = 0x02;
pub const EP_DATA_IN: u8 = 0x81;
pub const EP_MSG_IN: u8 = 0x82;
pub const EP_STAT_IN: u8 = 0x83;

// ─── USB identity ─────────────────────────────────────────────────────────────

pub const APEX_VID: u16 = 0x0878;
/// Running/application-mode product IDs (device exposes the 5-endpoint CAN
/// interface).  Everything else under the vendor ID is a bootloader.
///
/// `0x1101`/`0x1181` are the bench unit; `0x1122` is another USB-CANmodul1
/// hardware variant that boots straight into the same running-mode interface.
pub const PID_RUNNING: [u16; 3] = [0x1101, 0x1181, 0x1122];

// ─── Bootloader EP0 vendor requests ───────────────────────────────────────────
//
// Only RECONNECT is needed to boot a device whose flashed firmware is already
// current: it makes the bootloader jump to the application, after which the
// device re-enumerates with a running-mode PID.  (Flashing new firmware would
// also use READ_VERSION / START/WRITE/STOP_UPDATE / WRITE_FLASH / CHECK_CRC,
// which we deliberately do not implement — firmware is provisioned elsewhere.)

pub const VRREQ_READ_VERSION: u8 = 0xB0;
pub const VRREQ_RECONNECT: u8 = 0xB6;

// ─── Command opcodes (byte 0 of an 8-byte MSG) ────────────────────────────────

pub const CMD_INITIALIZE: u8 = 1;
pub const CMD_SHUTDOWN: u8 = 4;
pub const CMD_RESET: u8 = 5;
pub const CMD_READ_EEPROM: u8 = 6;
pub const CMD_SET_AMR: u8 = 11;
pub const CMD_SET_ACR: u8 = 12;
pub const CMD_SET_CAN_MODE: u8 = 13;
pub const CMD_SET_BAUDRATE_EX: u8 = 25;
/// Device sets this bit in the opcode byte of a command reply.
pub const CMD_REPLY_FLAG: u8 = 0x80;
pub const CMD_SIZE: usize = 8;

// ─── CAN-mode flags (arg of CMD_SET_CAN_MODE) ─────────────────────────────────

pub const MODE_NORMAL: u8 = 0x00;
pub const MODE_LISTEN_ONLY: u8 = 0x01;
pub const MODE_TX_ECHO: u8 = 0x02;
pub const MODE_ONE_SHOT: u8 = 0x10;

// ─── Frame format byte ────────────────────────────────────────────────────────

pub const FRAME_SIZE: usize = 16;
const FF_DLC_MASK: u8 = 0x0F;
const FF_RTR: u8 = 0x40;
const FF_EXT: u8 = 0x80;
/// A data record whose format byte equals this is the "end of reset" marker the
/// device emits once after INITIALIZE (CAN-ID field is 0xFFF0); it is not a real
/// frame and must be skipped.
const FF_END_OF_RESET: u8 = 0x0F;

const STD_ID_SHIFT: u32 = 5;
const EXT_ID_SHIFT: u32 = 3;

// ─── Bit timing ───────────────────────────────────────────────────────────────

/// Bit-timing register (`baud_ex_reg`) for a given nominal bitrate.
///
/// All nine standard rates are hardware-verified from bench captures (CAN clock
/// 24 MHz).  Layout: `0x40 << 24 | BTR1 << 16 | BTR0`, SJA1000-style.  Rates up
/// to 500k use a 16 tq bit (87.5% sample point); 800k/1M use tighter timing.
pub fn baud_ex_reg(bitrate: u32) -> Option<u32> {
    Some(match bitrate {
        10_000 => 0x401c_0095,
        20_000 => 0x401c_004a,
        50_000 => 0x401c_001d,
        100_000 => 0x401c_000e,
        125_000 => 0x401c_000b,
        250_000 => 0x401c_0005,
        500_000 => 0x401c_0002,
        800_000 => 0x402a_0001,
        1_000_000 => 0x4027_0001,
        _ => return None,
    })
}

// ─── Decoded frame ────────────────────────────────────────────────────────────

/// A CAN frame as carried on the DATA endpoints, decoded into plain fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApexFrame {
    pub id: u32,
    pub extended: bool,
    pub rtr: bool,
    pub dlc: u8,
    pub data: [u8; 8],
    /// 24-bit device timestamp (monotonic tick counter); unit TBD.
    pub timestamp: u32,
}

/// A decoded DATA-IN record: either a real frame or the end-of-reset marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataRecord {
    Frame(ApexFrame),
    EndOfReset,
}

/// Decode one 16-byte DATA-IN record.
pub fn decode_frame(buf: &[u8; FRAME_SIZE]) -> DataRecord {
    let format = buf[0];
    if format == FF_END_OF_RESET {
        return DataRecord::EndOfReset;
    }

    let extended = format & FF_EXT != 0;
    let rtr = format & FF_RTR != 0;
    let dlc = format & FF_DLC_MASK;

    let (id, data_off) = if extended {
        let raw = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]);
        (raw >> EXT_ID_SHIFT, 5)
    } else {
        let raw = u16::from_be_bytes([buf[1], buf[2]]) as u32;
        (raw >> STD_ID_SHIFT, 3)
    };

    let mut data = [0u8; 8];
    let n = (dlc as usize).min(8);
    data[..n].copy_from_slice(&buf[data_off..data_off + n]);

    // Timestamp is the trailing 3 bytes (big-endian 24-bit).
    let timestamp = u32::from_be_bytes([0, buf[13], buf[14], buf[15]]);

    DataRecord::Frame(ApexFrame {
        id,
        extended,
        rtr,
        dlc,
        data,
        timestamp,
    })
}

/// Encode a CAN frame into a 16-byte DATA-OUT record.
pub fn encode_frame(id: u32, extended: bool, rtr: bool, dlc: u8, data: &[u8]) -> [u8; FRAME_SIZE] {
    let mut buf = [0u8; FRAME_SIZE];
    let dlc = dlc.min(8);
    buf[0] = dlc | if extended { FF_EXT } else { 0 } | if rtr { FF_RTR } else { 0 };

    let data_off = if extended {
        buf[1..5].copy_from_slice(&(id << EXT_ID_SHIFT).to_be_bytes());
        5
    } else {
        let raw = ((id << STD_ID_SHIFT) & 0xFFFF) as u16;
        buf[1..3].copy_from_slice(&raw.to_be_bytes());
        3
    };

    if !rtr {
        let n = (dlc as usize).min(data.len());
        buf[data_off..data_off + n].copy_from_slice(&data[..n]);
    }
    buf
}

// ─── Command builders ─────────────────────────────────────────────────────────

fn cmd(op: u8, chan: u8) -> [u8; CMD_SIZE] {
    let mut c = [0u8; CMD_SIZE];
    c[0] = op;
    c[6] = chan;
    c
}

pub fn cmd_reset(chan: u8) -> [u8; CMD_SIZE] {
    cmd(CMD_RESET, chan)
}

pub fn cmd_initialize(chan: u8) -> [u8; CMD_SIZE] {
    cmd(CMD_INITIALIZE, chan)
}

pub fn cmd_shutdown(chan: u8) -> [u8; CMD_SIZE] {
    cmd(CMD_SHUTDOWN, chan)
}

/// SETAMR — acceptance mask (big-endian). `0xFFFFFFFF` accepts all IDs.
pub fn cmd_set_amr(chan: u8, mask: u32) -> [u8; CMD_SIZE] {
    let mut c = cmd(CMD_SET_AMR, chan);
    c[1..5].copy_from_slice(&mask.to_be_bytes());
    c
}

/// SETACR — acceptance code (big-endian).
pub fn cmd_set_acr(chan: u8, code: u32) -> [u8; CMD_SIZE] {
    let mut c = cmd(CMD_SET_ACR, chan);
    c[1..5].copy_from_slice(&code.to_be_bytes());
    c
}

pub fn cmd_set_can_mode(chan: u8, mode: u8) -> [u8; CMD_SIZE] {
    let mut c = cmd(CMD_SET_CAN_MODE, chan);
    c[1] = mode;
    c
}

/// SETBAUDRATE_EX — the 32-bit `baud_ex_reg` (little-endian).
pub fn cmd_set_baudrate_ex(chan: u8, baud_ex_reg: u32) -> [u8; CMD_SIZE] {
    let mut c = cmd(CMD_SET_BAUDRATE_EX, chan);
    c[1..5].copy_from_slice(&baud_ex_reg.to_le_bytes());
    c
}

/// The command sequence that brings a channel bus-on, in order.
///
/// Mirrors the observed open handshake: set bitrate, reset, accept-all filter,
/// CAN mode, then initialize (bus-on).
pub fn open_sequence(chan: u8, baud_ex_reg: u32, mode: u8) -> [[u8; CMD_SIZE]; 6] {
    [
        cmd_set_baudrate_ex(chan, baud_ex_reg),
        cmd_reset(chan),
        cmd_set_amr(chan, 0xFFFF_FFFF),
        cmd_set_acr(chan, 0x0000_0000),
        cmd_set_can_mode(chan, mode),
        cmd_initialize(chan),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures below are exact bytes from the Phase 0b bench capture
    // (issue #103), cross-checked against candump ground truth.

    #[test]
    fn decode_std_data_frame() {
        // 0x39C [8] 2B 8B 00 00 00 00 00 00  (candump), ts 0x0024da
        let buf = [
            0x08, 0x73, 0x80, 0x2b, 0x8b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x24, 0xda,
        ];
        let DataRecord::Frame(f) = decode_frame(&buf) else {
            panic!("expected frame");
        };
        assert_eq!(f.id, 0x39C);
        assert!(!f.extended && !f.rtr);
        assert_eq!(f.dlc, 8);
        assert_eq!(&f.data, &[0x2b, 0x8b, 0, 0, 0, 0, 0, 0]);
        assert_eq!(f.timestamp, 0x0024da);
    }

    #[test]
    fn decode_std_dlc6_frame() {
        // 0x35C [6] C5 FF CB FF 00 00
        let buf = [
            0x06, 0x6b, 0x80, 0xc5, 0xff, 0xcb, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x1d, 0x0b,
        ];
        let DataRecord::Frame(f) = decode_frame(&buf) else {
            panic!("expected frame");
        };
        assert_eq!(f.id, 0x35C);
        assert_eq!(f.dlc, 6);
        assert_eq!(&f.data[..6], &[0xc5, 0xff, 0xcb, 0xff, 0x00, 0x00]);
    }

    #[test]
    fn decode_end_of_reset_marker() {
        let buf = [
            0x0f, 0xff, 0xfc, 0xd3, 0x1c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x1c, 0xd3,
        ];
        assert_eq!(decode_frame(&buf), DataRecord::EndOfReset);
    }

    #[test]
    fn encode_std_dlc4() {
        // cansend 123#DEADBEEF -> 04 24 60 de ad be ef ...
        let out = encode_frame(0x123, false, false, 4, &[0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(&out[..7], &[0x04, 0x24, 0x60, 0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn encode_std_dlc8() {
        // cansend 601#4010200000000000 -> 08 c0 20 40 10 20 00 00 00 ...
        let out = encode_frame(0x601, false, false, 8, &[0x40, 0x10, 0x20, 0, 0, 0, 0, 0]);
        assert_eq!(
            &out[..11],
            &[0x08, 0xc0, 0x20, 0x40, 0x10, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn encode_decode_roundtrip_std() {
        let out = encode_frame(0x7FF, false, false, 8, &[1, 2, 3, 4, 5, 6, 7, 8]);
        let DataRecord::Frame(f) = decode_frame(&out) else {
            panic!("frame");
        };
        assert_eq!(f.id, 0x7FF);
        assert_eq!(f.dlc, 8);
        assert_eq!(&f.data, &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn encode_decode_roundtrip_ext() {
        let out = encode_frame(0x1ABCDEF, true, false, 3, &[0xaa, 0xbb, 0xcc]);
        assert_eq!(out[0], 0x83); // EXT | dlc 3
        let DataRecord::Frame(f) = decode_frame(&out) else {
            panic!("frame");
        };
        assert!(f.extended);
        assert_eq!(f.id, 0x1ABCDEF);
        assert_eq!(&f.data[..3], &[0xaa, 0xbb, 0xcc]);
    }

    #[test]
    fn set_baudrate_250k_matches_capture() {
        // dmesg: cmd buf 19 05 00 1c 40 00 00 00
        let reg = baud_ex_reg(250_000).unwrap();
        assert_eq!(reg, 0x401c_0005);
        assert_eq!(
            cmd_set_baudrate_ex(0, reg),
            [0x19, 0x05, 0x00, 0x1c, 0x40, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn baud_ex_reg_full_table_from_bench() {
        // Exact values dumped from hardware for every standard rate.
        let table = [
            (10_000u32, 0x401c_0095u32),
            (20_000, 0x401c_004a),
            (50_000, 0x401c_001d),
            (100_000, 0x401c_000e),
            (125_000, 0x401c_000b),
            (250_000, 0x401c_0005),
            (500_000, 0x401c_0002),
            (800_000, 0x402a_0001),
            (1_000_000, 0x4027_0001),
        ];
        for (br, reg) in table {
            assert_eq!(baud_ex_reg(br), Some(reg), "bitrate {br}");
        }
        assert_eq!(baud_ex_reg(33_333), None);
    }

    #[test]
    fn open_filter_and_mode_match_capture() {
        // 0b ff ff ff ff | 0c 00 .. | 0d 00 (normal)
        assert_eq!(
            cmd_set_amr(0, 0xFFFF_FFFF),
            [0x0b, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00]
        );
        assert_eq!(
            cmd_set_acr(0, 0),
            [0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        );
        assert_eq!(
            cmd_set_can_mode(0, MODE_NORMAL),
            [0x0d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        );
    }
}
