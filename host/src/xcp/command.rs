//! XCP command encoding and response decoding (CRO / DTO payloads).
//!
//! This mirrors the role of [`crate::canopen::sdo`] for CANopen: it turns
//! high-level master intents (CONNECT, UPLOAD, DOWNLOAD, …) into CRO byte
//! payloads, and decodes the slave's DTO responses back into typed values.
//!
//! Only the standard command set (STD) required by the RustyCAN master is
//! implemented for transmission. Passive [`decode_command`] additionally
//! recognises the DAQ configuration commands so the DAQ tracker (see
//! `crate::xcp::daq`) can rebuild measurement layouts from observed traffic.
//!
//! Addresses and multi-byte counters are encoded/decoded in the slave's
//! [`ByteOrder`], negotiated in the CONNECT response.

use super::ByteOrder;

// ─── Command codes (CRO byte 0) ─────────────────────────────────────────────

/// `CONNECT` — establish a connection with the slave.
pub const CONNECT: u8 = 0xFF;
/// `DISCONNECT` — terminate the connection.
pub const DISCONNECT: u8 = 0xFE;
/// `GET_STATUS` — read the current session status.
pub const GET_STATUS: u8 = 0xFD;
/// `SYNCH` — synchronise command processing after an error.
pub const SYNCH: u8 = 0xFC;
/// `GET_COMM_MODE_INFO` — read optional communication mode parameters.
pub const GET_COMM_MODE_INFO: u8 = 0xFB;
/// `GET_ID` — read slave identification.
pub const GET_ID: u8 = 0xFA;
/// `SET_REQUEST` — request a session-level action (e.g. store to NV memory).
pub const SET_REQUEST: u8 = 0xF9;
/// `GET_SEED` — request the seed for a protected resource (seed & key).
pub const GET_SEED: u8 = 0xF8;
/// `UNLOCK` — submit the computed key to unlock a resource.
pub const UNLOCK: u8 = 0xF7;
/// `SET_MTA` — set the Memory Transfer Address for subsequent UPLOAD/DOWNLOAD.
pub const SET_MTA: u8 = 0xF6;
/// `UPLOAD` — read `n` elements from the current MTA.
pub const UPLOAD: u8 = 0xF5;
/// `SHORT_UPLOAD` — read `n` elements from an address given in the same frame.
pub const SHORT_UPLOAD: u8 = 0xF4;
/// `BUILD_CHECKSUM` — compute a checksum over a memory block.
pub const BUILD_CHECKSUM: u8 = 0xF3;
/// `DOWNLOAD` — write elements to the current MTA.
pub const DOWNLOAD: u8 = 0xF0;
/// `DOWNLOAD_NEXT` — continue a block download.
pub const DOWNLOAD_NEXT: u8 = 0xEF;
/// `DOWNLOAD_MAX` — write a full MAX_CTO-sized block to the current MTA.
pub const DOWNLOAD_MAX: u8 = 0xEE;
/// `SHORT_DOWNLOAD` — write elements to an address given in the same frame.
pub const SHORT_DOWNLOAD: u8 = 0xED;
/// `MODIFY_BITS` — read-modify-write a masked word at the current MTA.
pub const MODIFY_BITS: u8 = 0xEC;

// DAQ configuration commands (recognised for passive tracking).
/// `CLEAR_DAQ_LIST` — clear a DAQ list configuration.
pub const CLEAR_DAQ_LIST: u8 = 0xE3;
/// `SET_DAQ_PTR` — select the DAQ list / ODT / ODT-entry to configure.
pub const SET_DAQ_PTR: u8 = 0xE2;
/// `WRITE_DAQ` — write one ODT entry (address + size) at the current DAQ pointer.
pub const WRITE_DAQ: u8 = 0xE1;
/// `SET_DAQ_LIST_MODE` — set mode / event channel for a DAQ list.
pub const SET_DAQ_LIST_MODE: u8 = 0xE0;
/// `START_STOP_DAQ_LIST` — start / stop / select a single DAQ list.
pub const START_STOP_DAQ_LIST: u8 = 0xDE;
/// `START_STOP_SYNCH` — start / stop all selected DAQ lists synchronously.
pub const START_STOP_SYNCH: u8 = 0xDD;
/// `GET_DAQ_CLOCK` — read the slave DAQ timestamp clock.
pub const GET_DAQ_CLOCK: u8 = 0xDC;

// ─── Command encoding (master → slave CRO payloads) ─────────────────────────

/// Encode a `CONNECT` command. `mode` is 0 for a normal connection or 1 for a
/// user-defined connection.
pub fn encode_connect(mode: u8) -> Vec<u8> {
    vec![CONNECT, mode]
}

/// Encode a `DISCONNECT` command.
pub fn encode_disconnect() -> Vec<u8> {
    vec![DISCONNECT]
}

/// Encode a `GET_STATUS` command.
pub fn encode_get_status() -> Vec<u8> {
    vec![GET_STATUS]
}

/// Encode a `GET_SEED` command for a protected resource.
///
/// `mode` selects the first (0) or remaining (1) seed bytes; `resource` is the
/// resource bitmask (e.g. calibration, DAQ, programming).
pub fn encode_get_seed(mode: u8, resource: u8) -> Vec<u8> {
    vec![GET_SEED, mode, resource]
}

/// Encode an `UNLOCK` command carrying up to the remaining key bytes.
///
/// `remaining` is the total number of key bytes still to be sent (per the XCP
/// spec, `UNLOCK` reports the outstanding length, not just this frame's slice).
pub fn encode_unlock(remaining: u8, key: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(2 + key.len());
    v.push(UNLOCK);
    v.push(remaining);
    v.extend_from_slice(key);
    v
}

/// Encode a `SET_MTA` command setting the Memory Transfer Address.
///
/// `addr_ext` is the address extension (segment/page selector, 0 when unused).
pub fn encode_set_mta(address: u32, addr_ext: u8, byte_order: ByteOrder) -> Vec<u8> {
    let a = byte_order.u32_to_bytes(address);
    vec![SET_MTA, 0x00, 0x00, addr_ext, a[0], a[1], a[2], a[3]]
}

/// Encode an `UPLOAD` command reading `n` elements from the current MTA.
pub fn encode_upload(n: u8) -> Vec<u8> {
    vec![UPLOAD, n]
}

/// Encode a `SHORT_UPLOAD` command reading `n` elements from `address`.
pub fn encode_short_upload(n: u8, address: u32, addr_ext: u8, byte_order: ByteOrder) -> Vec<u8> {
    let a = byte_order.u32_to_bytes(address);
    vec![SHORT_UPLOAD, n, 0x00, addr_ext, a[0], a[1], a[2], a[3]]
}

/// Encode a `DOWNLOAD` command writing `data` to the current MTA.
///
/// The element count is `data.len()`; the caller is responsible for keeping the
/// payload within the negotiated `MAX_CTO` (typically ≤ 6 data bytes on
/// classic CAN).
pub fn encode_download(data: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(2 + data.len());
    v.push(DOWNLOAD);
    v.push(data.len() as u8);
    v.extend_from_slice(data);
    v
}

// ─── Response decoding (slave → master DTO payloads) ────────────────────────

/// Address granularity reported in the CONNECT response — the size in bytes of
/// a single addressable element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AddressGranularity {
    /// 1 byte per element (`COMM_MODE_BASIC` bits 1..2 = 0).
    #[default]
    Byte,
    /// 2 bytes per element.
    Word,
    /// 4 bytes per element.
    Dword,
}

impl AddressGranularity {
    fn from_bits(bits: u8) -> Self {
        match bits & 0b11 {
            0 => AddressGranularity::Byte,
            1 => AddressGranularity::Word,
            _ => AddressGranularity::Dword,
        }
    }

    /// Number of bytes per addressable element.
    pub fn bytes(self) -> u8 {
        match self {
            AddressGranularity::Byte => 1,
            AddressGranularity::Word => 2,
            AddressGranularity::Dword => 4,
        }
    }
}

/// Decoded `CONNECT` positive response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectResponse {
    /// Resource-availability bitmask (calibration / DAQ / programming …).
    pub resource: u8,
    /// Slave byte order for multi-byte protocol fields.
    pub byte_order: ByteOrder,
    /// Address granularity (bytes per addressable element).
    pub address_granularity: AddressGranularity,
    /// Maximum CTO (command/response) packet length in bytes.
    pub max_cto: u8,
    /// Maximum DTO (data) packet length in bytes.
    pub max_dto: u16,
    /// XCP protocol layer version (major byte).
    pub protocol_version: u8,
    /// XCP transport layer version (major byte).
    pub transport_version: u8,
}

/// Decode a `CONNECT` positive response payload (DTO starting with 0xFF).
///
/// Returns `None` if the payload is too short or is not a positive response.
/// The `max_dto` field is read using the byte order embedded in the same
/// response's `COMM_MODE_BASIC` byte.
pub fn decode_connect_response(data: &[u8]) -> Option<ConnectResponse> {
    if data.first().copied()? != super::PID_RES || data.len() < 8 {
        return None;
    }
    let resource = data[1];
    let comm_mode_basic = data[2];
    let byte_order = if comm_mode_basic & 0x01 != 0 {
        ByteOrder::BigEndian
    } else {
        ByteOrder::LittleEndian
    };
    let address_granularity = AddressGranularity::from_bits(comm_mode_basic >> 1);
    let max_cto = data[3];
    let max_dto = byte_order.u16_from_bytes(&data[4..6]);
    Some(ConnectResponse {
        resource,
        byte_order,
        address_granularity,
        max_cto,
        max_dto,
        protocol_version: data[6],
        transport_version: data[7],
    })
}

/// XCP standard error codes carried in an `ERR` response (byte 1).
///
/// Only the common subset is named; unrecognised codes are surfaced verbatim by
/// [`error_code_name`].
pub mod error {
    /// Command processor synchronisation error.
    pub const ERR_CMD_SYNCH: u8 = 0x00;
    /// Command was not executed.
    pub const ERR_CMD_BUSY: u8 = 0x10;
    /// DAQ processor is busy.
    pub const ERR_DAQ_ACTIVE: u8 = 0x11;
    /// Program processor is busy.
    pub const ERR_PGM_ACTIVE: u8 = 0x12;
    /// Unknown / unsupported command.
    pub const ERR_CMD_UNKNOWN: u8 = 0x20;
    /// Command syntax invalid.
    pub const ERR_CMD_SYNTAX: u8 = 0x21;
    /// Parameter out of range.
    pub const ERR_OUT_OF_RANGE: u8 = 0x22;
    /// Memory / resource is write protected.
    pub const ERR_WRITE_PROTECTED: u8 = 0x23;
    /// Access denied — resource is locked (seed & key required).
    pub const ERR_ACCESS_LOCKED: u8 = 0x25;
    /// Access denied for the requested page.
    pub const ERR_PAGE_NOT_VALID: u8 = 0x26;
    /// Sequence error.
    pub const ERR_SEQUENCE: u8 = 0x29;
}

/// Human-readable name for an XCP error code (see [`error`]).
pub fn error_code_name(code: u8) -> &'static str {
    use error::*;
    match code {
        ERR_CMD_SYNCH => "ERR_CMD_SYNCH",
        ERR_CMD_BUSY => "ERR_CMD_BUSY",
        ERR_DAQ_ACTIVE => "ERR_DAQ_ACTIVE",
        ERR_PGM_ACTIVE => "ERR_PGM_ACTIVE",
        ERR_CMD_UNKNOWN => "ERR_CMD_UNKNOWN",
        ERR_CMD_SYNTAX => "ERR_CMD_SYNTAX",
        ERR_OUT_OF_RANGE => "ERR_OUT_OF_RANGE",
        ERR_WRITE_PROTECTED => "ERR_WRITE_PROTECTED",
        ERR_ACCESS_LOCKED => "ERR_ACCESS_LOCKED",
        ERR_PAGE_NOT_VALID => "ERR_PAGE_NOT_VALID",
        ERR_SEQUENCE => "ERR_SEQUENCE",
        _ => "ERR_UNKNOWN",
    }
}

// ─── Passive command decoding (for the sniffer & DAQ tracker) ───────────────

/// A decoded XCP command observed on the CRO identifier.
///
/// Multi-byte address / DAQ-pointer fields are interpreted using the supplied
/// [`ByteOrder`]. Commands outside the recognised set are returned as
/// [`XcpCommand::Other`] carrying the raw command code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XcpCommand {
    Connect {
        mode: u8,
    },
    Disconnect,
    GetStatus,
    Synch,
    GetCommModeInfo,
    GetId {
        id_type: u8,
    },
    SetRequest {
        mode: u8,
    },
    GetSeed {
        mode: u8,
        resource: u8,
    },
    Unlock {
        remaining: u8,
    },
    SetMta {
        addr_ext: u8,
        address: u32,
    },
    Upload {
        n: u8,
    },
    ShortUpload {
        n: u8,
        addr_ext: u8,
        address: u32,
    },
    BuildChecksum,
    Download {
        n: u8,
    },
    ShortDownload {
        n: u8,
        addr_ext: u8,
        address: u32,
    },
    ModifyBits,
    ClearDaqList {
        daq: u16,
    },
    SetDaqPtr {
        daq: u16,
        odt: u8,
        odt_entry: u8,
    },
    WriteDaq {
        bit_offset: u8,
        size: u8,
        addr_ext: u8,
        address: u32,
    },
    SetDaqListMode {
        mode: u8,
        daq: u16,
        event: u16,
        prescaler: u8,
        priority: u8,
    },
    StartStopDaqList {
        mode: u8,
        daq: u16,
    },
    StartStopSynch {
        mode: u8,
    },
    GetDaqClock,
    Other(u8),
}

/// Decode a CRO command payload into a typed [`XcpCommand`].
///
/// Returns `None` only for an empty payload. Address and DAQ-pointer fields use
/// `byte_order` (the slave's negotiated order).
pub fn decode_command(data: &[u8], byte_order: ByteOrder) -> Option<XcpCommand> {
    let code = *data.first()?;
    let b = |i: usize| data.get(i).copied().unwrap_or(0);
    let cmd = match code {
        CONNECT => XcpCommand::Connect { mode: b(1) },
        DISCONNECT => XcpCommand::Disconnect,
        GET_STATUS => XcpCommand::GetStatus,
        SYNCH => XcpCommand::Synch,
        GET_COMM_MODE_INFO => XcpCommand::GetCommModeInfo,
        GET_ID => XcpCommand::GetId { id_type: b(1) },
        SET_REQUEST => XcpCommand::SetRequest { mode: b(1) },
        GET_SEED => XcpCommand::GetSeed {
            mode: b(1),
            resource: b(2),
        },
        UNLOCK => XcpCommand::Unlock { remaining: b(1) },
        SET_MTA => XcpCommand::SetMta {
            addr_ext: b(3),
            address: byte_order.u32_from_bytes(&data[4.min(data.len())..]),
        },
        UPLOAD => XcpCommand::Upload { n: b(1) },
        SHORT_UPLOAD => XcpCommand::ShortUpload {
            n: b(1),
            addr_ext: b(3),
            address: byte_order.u32_from_bytes(&data[4.min(data.len())..]),
        },
        BUILD_CHECKSUM => XcpCommand::BuildChecksum,
        DOWNLOAD => XcpCommand::Download { n: b(1) },
        SHORT_DOWNLOAD => XcpCommand::ShortDownload {
            n: b(1),
            addr_ext: b(3),
            address: byte_order.u32_from_bytes(&data[4.min(data.len())..]),
        },
        MODIFY_BITS => XcpCommand::ModifyBits,
        CLEAR_DAQ_LIST => XcpCommand::ClearDaqList {
            daq: byte_order.u16_from_bytes(&data[2.min(data.len())..]),
        },
        SET_DAQ_PTR => XcpCommand::SetDaqPtr {
            daq: byte_order.u16_from_bytes(&data[2.min(data.len())..]),
            odt: b(4),
            odt_entry: b(5),
        },
        WRITE_DAQ => XcpCommand::WriteDaq {
            bit_offset: b(1),
            size: b(2),
            addr_ext: b(3),
            address: byte_order.u32_from_bytes(&data[4.min(data.len())..]),
        },
        SET_DAQ_LIST_MODE => XcpCommand::SetDaqListMode {
            mode: b(1),
            daq: byte_order.u16_from_bytes(&data[2.min(data.len())..]),
            event: byte_order.u16_from_bytes(&data[4.min(data.len())..]),
            prescaler: b(6),
            priority: b(7),
        },
        START_STOP_DAQ_LIST => XcpCommand::StartStopDaqList {
            mode: b(1),
            daq: byte_order.u16_from_bytes(&data[2.min(data.len())..]),
        },
        START_STOP_SYNCH => XcpCommand::StartStopSynch { mode: b(1) },
        GET_DAQ_CLOCK => XcpCommand::GetDaqClock,
        other => XcpCommand::Other(other),
    };
    Some(cmd)
}

/// Short human-readable mnemonic for a command code, for the sniffer's decode
/// column and log entries.
pub fn command_name(code: u8) -> &'static str {
    match code {
        CONNECT => "CONNECT",
        DISCONNECT => "DISCONNECT",
        GET_STATUS => "GET_STATUS",
        SYNCH => "SYNCH",
        GET_COMM_MODE_INFO => "GET_COMM_MODE_INFO",
        GET_ID => "GET_ID",
        SET_REQUEST => "SET_REQUEST",
        GET_SEED => "GET_SEED",
        UNLOCK => "UNLOCK",
        SET_MTA => "SET_MTA",
        UPLOAD => "UPLOAD",
        SHORT_UPLOAD => "SHORT_UPLOAD",
        BUILD_CHECKSUM => "BUILD_CHECKSUM",
        DOWNLOAD => "DOWNLOAD",
        DOWNLOAD_NEXT => "DOWNLOAD_NEXT",
        DOWNLOAD_MAX => "DOWNLOAD_MAX",
        SHORT_DOWNLOAD => "SHORT_DOWNLOAD",
        MODIFY_BITS => "MODIFY_BITS",
        CLEAR_DAQ_LIST => "CLEAR_DAQ_LIST",
        SET_DAQ_PTR => "SET_DAQ_PTR",
        WRITE_DAQ => "WRITE_DAQ",
        SET_DAQ_LIST_MODE => "SET_DAQ_LIST_MODE",
        START_STOP_DAQ_LIST => "START_STOP_DAQ_LIST",
        START_STOP_SYNCH => "START_STOP_SYNCH",
        GET_DAQ_CLOCK => "GET_DAQ_CLOCK",
        _ => "UNKNOWN",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_encodes_mode() {
        assert_eq!(encode_connect(0), vec![0xFF, 0x00]);
        assert_eq!(encode_connect(1), vec![0xFF, 0x01]);
    }

    #[test]
    fn set_mta_little_endian() {
        assert_eq!(
            encode_set_mta(0x1234_5678, 0x00, ByteOrder::LittleEndian),
            vec![0xF6, 0x00, 0x00, 0x00, 0x78, 0x56, 0x34, 0x12]
        );
    }

    #[test]
    fn set_mta_big_endian() {
        assert_eq!(
            encode_set_mta(0x1234_5678, 0x02, ByteOrder::BigEndian),
            vec![0xF6, 0x00, 0x00, 0x02, 0x12, 0x34, 0x56, 0x78]
        );
    }

    #[test]
    fn upload_and_short_upload() {
        assert_eq!(encode_upload(4), vec![0xF5, 0x04]);
        assert_eq!(
            encode_short_upload(4, 0x2000_0000, 0, ByteOrder::LittleEndian),
            vec![0xF4, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20]
        );
    }

    #[test]
    fn download_prefixes_length() {
        assert_eq!(encode_download(&[0xAA, 0xBB]), vec![0xF0, 0x02, 0xAA, 0xBB]);
    }

    #[test]
    fn unlock_carries_key() {
        assert_eq!(
            encode_unlock(4, &[1, 2, 3, 4]),
            vec![0xF7, 0x04, 1, 2, 3, 4]
        );
    }

    #[test]
    fn connect_response_little_endian() {
        // resource=0x15, comm_mode_basic=0x00 (LE, byte AG), max_cto=8,
        // max_dto=0x0008, proto=1, transport=1
        let r = decode_connect_response(&[0xFF, 0x15, 0x00, 0x08, 0x08, 0x00, 0x01, 0x01]).unwrap();
        assert_eq!(r.resource, 0x15);
        assert_eq!(r.byte_order, ByteOrder::LittleEndian);
        assert_eq!(r.address_granularity, AddressGranularity::Byte);
        assert_eq!(r.max_cto, 8);
        assert_eq!(r.max_dto, 8);
    }

    #[test]
    fn connect_response_big_endian_word_ag() {
        // comm_mode_basic bit0=1 (BE), bits1..2 = 01 (WORD)
        let r = decode_connect_response(&[0xFF, 0x01, 0x03, 0x08, 0x00, 0x40, 0x01, 0x01]).unwrap();
        assert_eq!(r.byte_order, ByteOrder::BigEndian);
        assert_eq!(r.address_granularity, AddressGranularity::Word);
        assert_eq!(r.max_dto, 0x0040);
    }

    #[test]
    fn connect_response_rejects_short_or_error() {
        assert!(decode_connect_response(&[0xFF, 0x00]).is_none());
        assert!(decode_connect_response(&[0xFE, 0x20]).is_none());
    }

    #[test]
    fn decode_set_mta_roundtrip() {
        let frame = encode_set_mta(0xDEAD_BEEF, 0x01, ByteOrder::LittleEndian);
        assert_eq!(
            decode_command(&frame, ByteOrder::LittleEndian),
            Some(XcpCommand::SetMta {
                addr_ext: 0x01,
                address: 0xDEAD_BEEF
            })
        );
    }

    #[test]
    fn decode_write_daq() {
        // WRITE_DAQ bit_offset=0xFF size=4 addr_ext=0 address=0x20000004 (LE)
        let frame = [0xE1, 0xFF, 0x04, 0x00, 0x04, 0x00, 0x00, 0x20];
        assert_eq!(
            decode_command(&frame, ByteOrder::LittleEndian),
            Some(XcpCommand::WriteDaq {
                bit_offset: 0xFF,
                size: 4,
                addr_ext: 0,
                address: 0x2000_0004
            })
        );
    }

    #[test]
    fn decode_set_daq_ptr() {
        // SET_DAQ_PTR daq=2 odt=1 odt_entry=0 (LE)
        let frame = [0xE2, 0x00, 0x02, 0x00, 0x01, 0x00];
        assert_eq!(
            decode_command(&frame, ByteOrder::LittleEndian),
            Some(XcpCommand::SetDaqPtr {
                daq: 2,
                odt: 1,
                odt_entry: 0
            })
        );
    }

    #[test]
    fn decode_unknown_command() {
        assert_eq!(
            decode_command(&[0x01], ByteOrder::LittleEndian),
            Some(XcpCommand::Other(0x01))
        );
        assert_eq!(decode_command(&[], ByteOrder::LittleEndian), None);
    }

    #[test]
    fn error_names() {
        assert_eq!(error_code_name(0x25), "ERR_ACCESS_LOCKED");
        assert_eq!(error_code_name(0xAB), "ERR_UNKNOWN");
    }
}
