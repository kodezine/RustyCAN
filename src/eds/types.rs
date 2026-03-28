use std::collections::HashMap;

/// CANopen data types as defined in CiA 301.
#[derive(Debug, Clone, PartialEq)]
pub enum DataType {
    Boolean,
    Integer8,
    Integer16,
    Integer32,
    Integer64,
    Unsigned8,
    Unsigned16,
    Unsigned32,
    Unsigned64,
    Real32,
    Real64,
    VisibleString,
    OctetString,
    Unknown(u16),
}

impl DataType {
    pub fn from_code(code: u16) -> Self {
        match code {
            0x0001 => DataType::Boolean,
            0x0002 => DataType::Integer8,
            0x0003 => DataType::Integer16,
            0x0004 => DataType::Integer32,
            0x0018 => DataType::Integer64,
            0x0005 => DataType::Unsigned8,
            0x0006 => DataType::Unsigned16,
            0x0007 => DataType::Unsigned32,
            0x001B => DataType::Unsigned64,
            0x0008 => DataType::Real32,
            0x0011 => DataType::Real64,
            0x0009 => DataType::VisibleString,
            0x000A => DataType::OctetString,
            other => DataType::Unknown(other),
        }
    }

    /// Nominal bit width for fixed-size types (None for strings/domain).
    pub fn bit_width(&self) -> Option<usize> {
        match self {
            DataType::Boolean | DataType::Integer8 | DataType::Unsigned8 => Some(8),
            DataType::Integer16 | DataType::Unsigned16 => Some(16),
            DataType::Integer32 | DataType::Unsigned32 | DataType::Real32 => Some(32),
            DataType::Integer64 | DataType::Unsigned64 | DataType::Real64 => Some(64),
            _ => None,
        }
    }
}

/// Access type for an OD entry.
#[derive(Debug, Clone, PartialEq)]
pub enum AccessType {
    ReadOnly,
    WriteOnly,
    ReadWrite,
    Const,
    Unknown,
}

impl AccessType {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().trim() {
            "ro" | "read" => AccessType::ReadOnly,
            "wo" | "write" => AccessType::WriteOnly,
            "rw" | "readwrite" => AccessType::ReadWrite,
            "const" => AccessType::Const,
            _ => AccessType::Unknown,
        }
    }
}

/// A single entry in the object dictionary.
#[derive(Debug, Clone)]
pub struct OdEntry {
    pub name: String,
    pub data_type: DataType,
    pub access: AccessType,
    /// Raw DefaultValue string from the EDS (hex or decimal).
    pub default_value: Option<String>,
}

/// Key: (object index, subindex). Subindex 0 for VAR objects with no subs.
pub type ObjectDictionary = HashMap<(u16, u8), OdEntry>;
