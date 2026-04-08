//! CAN adapter abstraction for RustyCAN.
//!
//! Provides a single trait [`CanAdapter`] that both the PEAK PCAN-USB and the
//! KCAN dongle implement.  The session layer only sees this trait — it has no
//! knowledge of which physical hardware is in use.
//!
//! # Adding an adapter
//!
//! 1. Create a new submodule (e.g. `my_adapter.rs`).
//! 2. Implement [`CanAdapter`] for your type.
//! 3. Add a variant to [`AdapterKind`].
//! 4. Handle it in [`open_adapter`].

use std::fmt;
use std::time::Duration;

use host_can::frame::CanFrame;

pub mod kcan;
// PEAK adapter uses host-can's pcan feature which is macOS/Windows only.
// On Linux, PEAK hardware is accessed via SocketCAN (kernel driver).
#[cfg(not(target_os = "linux"))]
pub mod peak;

// ─── Public types ─────────────────────────────────────────────────────────────

/// A CAN frame together with an optional hardware timestamp.
///
/// For the KCAN dongle, `hardware_timestamp_us` holds the FDCAN TIM2 value
/// captured in the ISR — sub-microsecond accuracy, no USB polling jitter.
///
/// For PEAK PCAN-USB, the field is `None` (host timestamps on USB receipt).
pub struct ReceivedFrame {
    pub frame: CanFrame,
    /// µs since dongle bus-on, captured in hardware.  `None` for PEAK.
    pub hardware_timestamp_us: Option<u32>,
}

/// Errors returned by adapter operations.
#[derive(Debug)]
pub enum AdapterError {
    /// The requested device was not found (wrong port or dongle unplugged).
    NotFound(String),
    /// The adapter returned a receive timeout (normal — not a hard error).
    Timeout,
    /// The underlying transport returned an error.
    Io(String),
    /// The KCAN protocol returned an unexpected response.
    Protocol(String),
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(s) => write!(f, "adapter not found: {s}"),
            Self::Timeout => write!(f, "receive timeout"),
            Self::Io(s) => write!(f, "I/O error: {s}"),
            Self::Protocol(s) => write!(f, "protocol error: {s}"),
        }
    }
}

/// Selects which adapter backend to use when opening a session.
#[derive(Debug, Clone)]
pub enum AdapterKind {
    /// PEAK PCAN-USB dongle accessed via `host-can` / libPCBUSB.
    ///
    /// `port` is the channel number string: `"1"` for PCAN_USBBUS1, etc.
    Peak,
    /// KCAN dongle connected over USB.
    ///
    /// `serial` optionally pins a specific dongle by its USB serial string.
    /// When `None`, the first KCAN device found is used.
    KCan { serial: Option<String> },
}

/// Uniform interface for sending and receiving CAN frames.
pub trait CanAdapter {
    /// Block until a frame is available or `timeout` elapses.
    ///
    /// Returns [`AdapterError::Timeout`] on a clean timeout — the caller
    /// should retry.  Any other error is a hard failure.
    fn recv(&mut self, timeout: Duration) -> Result<ReceivedFrame, AdapterError>;

    /// Transmit a CAN frame.
    fn send(&mut self, frame: &CanFrame) -> Result<(), AdapterError>;

    /// Human-readable adapter name for log messages and UI display.
    fn name(&self) -> &str;
}

// ─── Factory ──────────────────────────────────────────────────────────────────

/// Open the adapter described by `kind`.
///
/// Called from the session recv thread (the adapter is created on the thread
/// that will use it — some backends are not `Sync`).
pub fn open_adapter(
    kind: &AdapterKind,
    port: &str,
    baud: u32,
    listen_only: bool,
) -> Result<Box<dyn CanAdapter>, AdapterError> {
    match kind {
        AdapterKind::Peak => {
            #[cfg(not(target_os = "linux"))]
            {
                let inner = host_can::adapter::get_adapter(port, baud).map_err(|e| {
                    let detail = e.to_string();
                    // libloading surfaces a "cannot open shared object" / "dlopen" message
                    // when libPCBUSB.dylib / PCANBasic.dll is not installed.
                    if detail.to_lowercase().contains("libpcbusb")
                        || detail.to_lowercase().contains("pcanbasic")
                        || detail.to_lowercase().contains("dlopen")
                        || detail.to_lowercase().contains("cannot open shared")
                        || detail.to_lowercase().contains("the specified module")
                    {
                        AdapterError::NotFound(format!(
                            "PEAK driver library not found. \
                            Please install the PCANBasic driver:\n\
                            • macOS: https://mac-can.com\n\
                            • Windows: https://peak-system.com/downloads\n\
                            ({detail})"
                        ))
                    } else {
                        AdapterError::NotFound(detail)
                    }
                })?;
                Ok(Box::new(peak::PeakAdapter::new(inner)))
            }
            #[cfg(target_os = "linux")]
            Err(AdapterError::NotFound(
                "PEAK PCAN-USB is not supported on Linux via the proprietary driver. \
                Use the KCAN dongle instead, or connect via SocketCAN."
                    .into(),
            ))
        }
        AdapterKind::KCan { serial } => {
            let adapter = kcan::KCanAdapter::open(serial.as_deref(), baud, listen_only)?;
            Ok(Box::new(adapter))
        }
    }
}

/// Probe whether an adapter is reachable without starting a session.
///
/// Used by the Connect-screen polling loop.
pub fn probe_adapter_kind(kind: &AdapterKind, port: &str, baud: u32) -> bool {
    match kind {
        AdapterKind::Peak => {
            #[cfg(not(target_os = "linux"))]
            {
                host_can::adapter::get_adapter(port, baud).is_ok()
            }
            #[cfg(target_os = "linux")]
            false
        }
        AdapterKind::KCan { serial } => kcan::KCanAdapter::probe(serial.as_deref()),
    }
}
