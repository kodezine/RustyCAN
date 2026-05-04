//! Pre-converted 128×128 RGB565 splash icon.
//!
//! Generated at build time from `host/assets/RustyCAN.iconset/icon_128x128.png`
//! by `lcd-terminal/build.rs`.  Alpha is pre-multiplied against black.

/// RustyCAN 128×128 icon in packed RGB565 (row-major, no stride).
///
/// 16 384 pixels × 2 bytes = 32 768 bytes stored in flash.
pub static ICON: [u16; 128 * 128] = include!(concat!(env!("OUT_DIR"), "/icon_128x128_rgb565.rs"));

pub const ICON_W: u16 = 128;
pub const ICON_H: u16 = 128;
