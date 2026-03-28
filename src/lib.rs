/// Public library surface for RustyCAN.
///
/// Exposes the EDS parser, CANopen protocol decoders, and logger so they can
/// be used by integration tests and external tools.
pub mod canopen;
pub mod eds;
pub mod logger;
pub mod tui;
