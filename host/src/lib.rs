//! Public library surface for RustyCAN.
//!
//! Exposes the EDS parser, CANopen protocol decoders, logger, and DBC parser
//! so they can be used by integration tests and external tools.

/// Ed25519 public key for KCAN firmware signature verification.
///
/// Embedded from `firmware/signing-pubkey.bin` at compile time.  Must match
/// the private key used by `sign-firmware` and compiled into the bootloader.
pub static KCAN_SIGNING_PUBKEY: [u8; 32] = *include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../firmware/signing-pubkey.bin"
));

/// Parse the `RUSTYCAN_VERSION` build-time string (e.g. `"v0.2.0"` or
/// `"v0.2.0-3-gabcdef"`) into a `(major, minor, patch)` triple.
///
/// Returns `None` when the string cannot be parsed (e.g. in CI without tags).
pub fn bundled_firmware_version() -> Option<(u8, u8, u8)> {
    let ver = env!("RUSTYCAN_VERSION");
    let ver = ver.trim().trim_start_matches('v');
    // Take only the vX.Y.Z prefix, ignoring any `-N-gHASH` describe suffix.
    let base = ver.split('-').next()?;
    let mut parts = base.splitn(3, '.');
    let maj: u8 = parts.next()?.parse().ok()?;
    let min: u8 = parts.next()?.parse().ok()?;
    let pat: u8 = parts.next()?.parse().ok()?;
    Some((maj, min, pat))
}

pub mod adapters;
pub mod app;
pub mod canopen;
pub mod dbc;
pub mod dfu;
pub mod eds;
pub mod gui;
pub mod http_server;
pub mod logger;
pub mod session;
pub mod tui;
