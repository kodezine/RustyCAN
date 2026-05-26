//! CANopen Emergency (EMCY) frame decoder — CiA 301 §7.2.7.
//!
//! Emergency objects are transmitted by a device when an internal error occurs.
//! COB-ID = 0x080 + node_id.  Payload layout:
//!
//! | Bytes | Field           |
//! |-------|-----------------|
//! | 0–1   | Error code (LE) |
//! | 2     | Error register  |
//! | 3–7   | Vendor-specific data (classic CAN: 5 bytes; CAN FD: up to 61 bytes) |

/// A decoded CANopen Emergency event.
#[derive(Debug, Clone)]
pub struct EmcyEvent {
    pub node_id: u8,
    /// CiA 301 error code (bytes 0–1 of the payload, little-endian).
    pub error_code: u16,
    /// CiA 301 error register (byte 2, mirrors object 0x1001).
    pub error_register: u8,
    /// Vendor-specific data (bytes 3 onwards, up to 61 bytes for CAN FD).
    pub vendor_data: Vec<u8>,
}

/// Decode a raw EMCY frame payload for `node_id`.
///
/// Returns `None` when the payload is shorter than 3 bytes (minimum
/// required: error code 2 bytes + error register 1 byte).
pub fn decode_emcy(node_id: u8, data: &[u8]) -> Option<EmcyEvent> {
    if data.len() < 3 {
        return None;
    }
    let error_code = u16::from_le_bytes([data[0], data[1]]);
    let error_register = data[2];
    let vendor_data = data[3..].to_vec();
    Some(EmcyEvent {
        node_id,
        error_code,
        error_register,
        vendor_data,
    })
}

/// Return a human-readable description for a CiA 301 error code.
///
/// Covers the standard error classes defined in CiA 301 Table 26.
/// Returns `"Unknown"` for codes not in the table.
pub fn describe_error_code(code: u16) -> &'static str {
    // Exact matches first, then class-level ranges.
    match code {
        0x0000 => "No error",
        // Generic errors 0x1xxx
        0x1000 => "Generic error",
        0x1001 => "Generic I/O error",
        // Current errors 0x2xxx
        0x2000 => "Current — generic",
        0x2100 => "Current (device input side)",
        0x2200 => "Current inside device",
        0x2300 => "Current (device output side)",
        // Voltage errors 0x3xxx
        0x3000 => "Voltage — generic",
        0x3100 => "Mains voltage",
        0x3200 => "Voltage inside device",
        0x3300 => "Output voltage",
        // Temperature errors 0x4xxx
        0x4000 => "Temperature — generic",
        0x4100 => "Ambient temperature",
        0x4200 => "Device temperature",
        // Hardware errors 0x5xxx
        0x5000 => "Device hardware — generic",
        0x5100 => "Device hardware — memory",
        0x5200 => "Device hardware — peripheral",
        0x5400 => "Device hardware — output stage",
        0x5500 => "Device hardware — monitoring",
        // Software errors 0x6xxx
        0x6000 => "Device software — generic",
        0x6100 => "Internal software",
        0x6200 => "User software",
        0x6300 => "Data set",
        // Module / SRDO errors 0x7xxx
        0x7000 => "Additional modules — generic",
        // Monitoring errors 0x8xxx
        0x8000 => "Monitoring — generic",
        0x8100 => "Communication — generic",
        0x8110 => "CAN overrun (objects lost)",
        0x8120 => "CAN in error passive mode",
        0x8130 => "Life guard or heartbeat error",
        0x8140 => "Recovered from bus-off",
        0x8150 => "CAN-ID collision",
        0x8200 => "Protocol error — generic",
        0x8210 => "PDO not processed due to length error",
        0x8220 => "PDO length exceeded",
        0x8230 => "DAM MPDO not processed, destination object not available",
        0x8240 => "Unexpected SYNC data length",
        0x8250 => "RPDO timeout",
        // External errors 0x9xxx
        0x9000 => "External error — generic",
        // Additional functions 0xFxxx
        0xF000 => "Additional functions — generic",
        0xFF00 => "Device specific — generic",
        _ => match code >> 8 {
            0x10..=0x1F => "Generic error",
            0x20..=0x2F => "Current error",
            0x30..=0x3F => "Voltage error",
            0x40..=0x4F => "Temperature error",
            0x50..=0x5F => "Device hardware error",
            0x60..=0x6F => "Device software error",
            0x70..=0x7F => "Additional modules error",
            0x80..=0x8F => "Monitoring / communication error",
            0x90..=0x9F => "External error",
            0xF0..=0xFF => "Additional / device-specific error",
            _ => "Unknown",
        },
    }
}

/// Format the error register byte as a human-readable bit list.
///
/// Returns a comma-separated string of active bit names per CiA 301 §7.2.1.
pub fn describe_error_register(reg: u8) -> String {
    const BITS: &[(u8, &str)] = &[
        (0, "Generic"),
        (2, "Voltage"),
        (3, "Temperature"),
        (4, "Communication"),
        (5, "Device Profile"),
        (7, "Manufacturer"),
    ];
    let active: Vec<&str> = BITS
        .iter()
        .filter(|(bit, _)| reg & (1 << bit) != 0)
        .map(|(_, name)| *name)
        .collect();
    if active.is_empty() {
        "None".to_string()
    } else {
        active.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_basic_emcy() {
        let data = [0x10, 0x81, 0x01, 0xDE, 0xAD, 0xBE, 0xEF, 0x00];
        let ev = decode_emcy(5, &data).unwrap();
        assert_eq!(ev.node_id, 5);
        assert_eq!(ev.error_code, 0x8110);
        assert_eq!(ev.error_register, 0x01);
        assert_eq!(ev.vendor_data, vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00]);
    }

    #[test]
    fn decode_no_error() {
        let data = [0x00, 0x00, 0x00];
        let ev = decode_emcy(1, &data).unwrap();
        assert_eq!(ev.error_code, 0x0000);
        assert_eq!(describe_error_code(ev.error_code), "No error");
    }

    #[test]
    fn decode_too_short() {
        assert!(decode_emcy(1, &[0x00, 0x00]).is_none());
    }

    #[test]
    fn describe_register_bits() {
        let s = describe_error_register(0b10010001); // bits 0, 4, 7
        assert!(s.contains("Generic"));
        assert!(s.contains("Communication"));
        assert!(s.contains("Manufacturer"));
    }
}
