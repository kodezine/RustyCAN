//! Types for decoded DBC (CAN database) signals.
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
}
