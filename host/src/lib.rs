/// Public library surface for RustyCAN.
///
/// Exposes the EDS parser, CANopen protocol decoders, logger, and DBC parser
/// so they can be used by integration tests and external tools.
pub mod adapters;
pub mod app;
pub mod canopen;
pub mod dbc;
pub mod eds;
pub mod gui;
pub mod http_server;
pub mod logger;
pub mod session;
