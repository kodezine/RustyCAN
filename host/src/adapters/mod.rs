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
/// For the KCAN dongle, `hardware_timestamp_ns` holds the FDCAN RXTS value
/// latched at frame SOF (100 ns resolution, embassy 10 MHz tick rate).
/// The host `TsRolloverTracker` in session extends this to a monotonic u64.
///
/// For PEAK PCAN-USB, the field is `None` (host timestamps on USB receipt).
pub struct ReceivedFrame {
    pub frame: CanFrame,
    /// Nanoseconds since dongle bus-on, latched at frame SOF.  `None` for PEAK.
    pub hardware_timestamp_ns: Option<u64>,
    /// Source CAN channel: 0 = FDCAN1, 1 = FDCAN2.  Always 0 for PEAK.
    pub channel: u8,
    /// `true` when this is a TX echo returned by the dongle after a successful
    /// frame transmission.  The `hardware_timestamp_ns` is the moment the last
    /// bit left the bus.  Always `false` for PEAK (no echo mechanism).
    pub is_tx_echo: bool,
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
    /// Unrecoverable error — the session must be terminated.
    Fatal(String),
    /// The USB device was physically disconnected.  The session may attempt
    /// to reconnect rather than terminating.
    Disconnected,
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(s) => write!(f, "adapter not found: {s}"),
            Self::Timeout => write!(f, "receive timeout"),
            Self::Io(s) => write!(f, "I/O error: {s}"),
            Self::Protocol(s) => write!(f, "protocol error: {s}"),
            Self::Fatal(s) => write!(f, "fatal error: {s}"),
            Self::Disconnected => write!(f, "USB device disconnected"),
        }
    }
}

/// Selects which adapter backend to use when opening a session.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

    /// Firmware version reported by the device during open, if available.
    ///
    /// Returns `Some((major, minor, patch))` for KCAN dongles; `None` for
    /// all other adapters (PEAK, virtual, etc.).
    fn firmware_version(&self) -> Option<(u8, u8, u8)> {
        None
    }
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
                // Prevent libPCBUSB.dylib from ever being dlclose'd.
                //
                // libPCBUSB 0.13 starts an IOKit CFRunLoop thread (USB plug-in
                // detection) as soon as the library is loaded via dlopen.  If
                // libloading subsequently calls dlclose (e.g. because CAN_Initialize
                // returned an error and the PcanAdapter was never constructed, or
                // because the PcanAdapter was dropped), that thread's code and its
                // CFMachPort callback are unmapped.  Any later USB plug/unplug event
                // fires the stale callback at the now-unmapped address → SIGBUS.
                //
                // RTLD_NODELETE (0x80 on macOS) tells dyld to never unmap the
                // library even when dlclose reduces its refcount to zero.  We set
                // this flag unconditionally before get_adapter() opens the library
                // for the first time; subsequent open/close cycles only adjust the
                // refcount without ever reaching 0.
                #[cfg(target_os = "macos")]
                {
                    use std::sync::Once;
                    static NODELETE_INIT: Once = Once::new();
                    NODELETE_INIT.call_once(|| {
                        let name = b"libPCBUSB.dylib\0";
                        unsafe {
                            // RTLD_LAZY (0x01) | RTLD_GLOBAL (0x08) | RTLD_NODELETE (0x80)
                            let _handle = libc::dlopen(
                                name.as_ptr() as *const libc::c_char,
                                0x01 | 0x08 | 0x80,
                            );
                            // Intentionally leak _handle: the RTLD_NODELETE flag is
                            // already stored in dyld's state; dropping the handle
                            // would just call dlclose once more (harmless but noisy).
                        }
                    });
                }
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
///
/// # PEAK probing strategy
///
/// Do NOT use `host_can::adapter::get_adapter()` to probe for PEAK hardware.
/// That function opens the PCAN channel (CAN_Initialize) and then immediately
/// drops it (CAN_Uninitialize + dlclose).  The macOS PEAK driver
/// (libPCBUSB.dylib) starts internal USB callback threads on CAN_Initialize;
/// dlclose frees the library's text segment while those threads are still
/// running, producing a SIGSEGV on the next open.
///
/// Instead, detect PEAK hardware by scanning USB devices for PEAK System's
/// vendor ID (0x0C72), which is safe to call repeatedly from any thread.
pub fn probe_adapter_kind(kind: &AdapterKind, _port: &str, _baud: u32) -> bool {
    match kind {
        AdapterKind::Peak => {
            #[cfg(target_os = "macos")]
            {
                // nusb::list_devices() on macOS 26 Tahoe triggers a stack
                // overflow in libusb's IOKit CFRunLoop thread when a PEAK
                // adapter is present (libusb + Tahoe + PEAK USB interaction
                // bug).  Use ioreg via subprocess to avoid touching the USB
                // device directly from this process.
                std::process::Command::new("ioreg")
                    .args(["-p", "IOUSB", "-l", "-w0"])
                    .output()
                    .map(|out| {
                        let stdout = String::from_utf8_lossy(&out.stdout);
                        // ioreg prints idVendor as a decimal integer.
                        // PEAK System VID = 0x0C72 = 3186 decimal.
                        stdout.contains("\"idVendor\" = 3186")
                    })
                    .unwrap_or(false)
            }
            #[cfg(target_os = "windows")]
            {
                // On Windows use nusb to enumerate USB devices and check for
                // the PEAK System vendor ID (0x0C72 = 3186).  The macOS Tahoe
                // nusb stack-overflow bug does not affect Windows.
                use nusb::MaybeFuture as _;
                const PEAK_VID: u16 = 0x0C72;
                nusb::list_devices()
                    .wait()
                    .map(|mut iter| iter.any(|d| d.vendor_id() == PEAK_VID))
                    .unwrap_or(false)
            }
            #[cfg(target_os = "linux")]
            false
        }
        AdapterKind::KCan { serial } => kcan::KCanAdapter::probe(serial.as_deref()),
    }
}
