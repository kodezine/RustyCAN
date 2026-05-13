//! Types for decoded DBC (CAN database) signals.

/// Byte order (endianness) of a DBC signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbcByteOrder {
    /// Intel (little-endian): `start_bit` is the LSBit position.
    LittleEndian,
    /// Motorola (big-endian): `start_bit` is the MSBit position in DBC numbering.
    BigEndian,
}

/// Integer interpretation for a DBC signal's raw bit pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbcValueType {
    /// Raw bits are treated as an unsigned integer.
    Unsigned,
    /// Raw bits are treated as a two's-complement signed integer.
    Signed,
}

/// Encoding metadata needed to write a physical value back into a CAN frame payload.
///
/// Stored alongside decoded values in [`DbcSignalValue`] so the GUI can
/// construct outbound frames without a separate database lookup.
#[derive(Debug, Clone)]
pub struct DbcSignalDef {
    /// LSBit (Intel) or MSBit (Motorola) position in the DBC bit numbering.
    pub start_bit: u64,
    /// Number of bits occupied by this signal.
    pub bit_size: u64,
    /// Byte order used when packing / unpacking.
    pub byte_order: DbcByteOrder,
    /// Signed or unsigned interpretation of the raw bits.
    pub value_type: DbcValueType,
    /// Scale factor: `physical = raw * factor + offset`.
    pub factor: f64,
    /// Offset: `physical = raw * factor + offset`.
    pub offset: f64,
    /// DLC of the containing message in bytes.
    pub dlc: u8,
}

/// A single decoded signal value from a CAN frame.
#[derive(Debug, Clone)]
pub struct DbcSignalValue {
    /// Signal name as defined in the DBC file.
    pub signal_name: String,
    /// Raw integer value extracted from the CAN frame payload.
    pub raw_int: i64,
    /// Physical value after applying factor and offset: `raw * factor + offset`.
    pub physical: f64,
    /// Engineering unit string (e.g. `"rpm"`, `"°C"`, `""`).
    pub unit: String,
    /// Optional human-readable description from a `VAL_` entry (e.g. `"Off"`, `"Active"`).
    pub description: Option<String>,
    /// Encoding metadata for writing this signal back into a CAN frame.
    ///
    /// `None` only when the signal was decoded without metadata (legacy path).
    pub encoding_def: Option<DbcSignalDef>,
}

/// All decoded signals from one CAN message (frame).
#[derive(Debug, Clone)]
pub struct DbcFrameSignals {
    /// Message name as defined in the DBC file.
    pub message_name: String,
    /// Raw CAN ID (without the EFF bit; always the numeric message ID).
    pub can_id: u32,
    /// Decoded signals in DBC definition order.
    pub values: Vec<DbcSignalValue>,
    /// Source DBC file stem (e.g. "vehicle_bus") for traceability in multi-DBC setups.
    pub source_dbc: String,
    /// Raw bytes of the original CAN frame payload.
    pub raw_data: Vec<u8>,
}
