//! CAN adapter abstraction for RustyCAN.
//!
//! Provides a single trait [`CanAdapter`] that every hardware backend
//! implements.  The session layer only sees this trait — it has no knowledge of
//! which physical hardware is in use.
//!
//! # One instance, one adapter
//!
//! Each running RustyCAN process owns **exactly one** adapter for the lifetime
//! of a session.  Multi-bus capture requires multiple RustyCAN instances, each
//! launched with its own `--config` file.  Adapter ownership is enforced:
//!
//! - **USB adapters** (KCan, Apex): OS-level exclusive claim via `nusb`.
//! - **Summit**: `CAN_Initialize()` returns an error if already opened.
//! - **KCanNet**: kgate's 1-to-1 room pairing prevents a second host from
//!   stealing an active session (ADR-0019 in the rustyepd repository).
//! - **SocketCAN**: application-level PID lockfile (planned, Issue E).
//!
//! The connect UI reflects three probe states: `Available`, `InUse`, `Absent`
//! (`registry::AdapterAvailability`, planned — Issue B).
//!
//! # Timestamp accuracy
//!
//! Two tiers exist; the distinction is visible in [`ReceivedFrame`]:
//!
//! | Adapter | `hardware_timestamp_ns` | Origin |
//! |---------|------------------------|--------|
//! | KCan USB | `Some(t)` | FDCAN RXTS latched at CAN frame SOF |
//! | KCanNet | `Some(t)` | Same FDCAN RXTS, frozen before TCP transit |
//! | Summit, Apex, SocketCAN | `None` | Host wall-clock on USB receipt |
//!
//! The UI renders `≈` for host-approximate timestamps.
//!
//! # Adding an adapter
//!
//! 1. Create a new submodule (e.g. `my_adapter.rs`).
//! 2. Implement [`CanAdapter`] for your type.
//! 3. Add a variant to [`AdapterKind`] with all identity fields in the variant.
//! 4. Handle it in [`open_adapter`].

use std::fmt;
use std::time::Duration;

use host_can::frame::CanFrame;

pub mod kcan;
// Pluggable adapter registry (issue #111) — step 1 scaffolding, delegates to
// the enum-based `open_adapter` for now.
pub mod registry;
// KCan-over-TCP: encrypted session bootstrapped from the device panel QR code.
pub mod kcan_net;
// Summit adapter uses host-can's pcan feature which is macOS/Windows only.
// On Linux, Summit hardware is accessed via SocketCAN (kernel driver).
#[cfg(not(target_os = "linux"))]
pub mod summit;
// SocketCAN adapter is Linux-only — uses the kernel's raw CAN socket API.
#[cfg(target_os = "linux")]
pub mod socketcan_adapter;
// Apex USB-CAN — cross-platform userspace USB driver (nusb).
pub mod apex;

// ─── Public types ─────────────────────────────────────────────────────────────

/// A CAN frame together with an optional hardware timestamp.
///
/// ## Timestamp accuracy
///
/// `hardware_timestamp_ns` is `Some` only for KCAN adapters (USB and Net).
/// It holds the FDCAN RXTS value latched at CAN frame SOF (100 ns resolution,
/// embassy 10 MHz tick rate).  The value is frozen in firmware before the frame
/// enters any transport path, so **network relay latency does not affect it**.
/// KCanNet timestamps are therefore as accurate as KCanUsb timestamps.
///
/// For all other adapters (Summit, Apex, SocketCAN) the field is `None`;
/// the session layer falls back to a host wall-clock timestamp on USB receipt.
/// These are subject to USB polling jitter and OS scheduling latency (~1–50 ms).
/// `SO_TIMESTAMPING` on SocketCAN is not pursued — the gain is marginal and
/// does not close the gap to FDCAN SOF-latched accuracy.
///
/// The `TsRolloverTracker` in `session` reconstructs a monotonic `u64` from
/// the 16-bit RXTS (wraps at ~6.55 ms) for each adapter independently.
pub struct ReceivedFrame {
    pub frame: CanFrame,
    /// `Some`: FDCAN SOF-latched, 100 ns resolution (KCan USB and Net only).
    /// `None`: host wall-clock on USB receipt (Summit, Apex, SocketCAN).
    pub hardware_timestamp_ns: Option<u64>,
    /// Source CAN channel: 0 = FDCAN1, 1 = FDCAN2.  Always 0 for single-channel adapters.
    pub channel: u8,
    /// `true` when this is a TX echo confirming a successful transmission.
    /// `hardware_timestamp_ns` is then the moment the last bit left the bus.
    /// Only KCAN adapters produce TX echoes; Summit, Apex, and SocketCAN do not.
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
    /// The adapter's transmit queue is momentarily full; the caller may back
    /// off and retry.  Distinct from `Io`, which signals a real send failure.
    TxQueueFull,
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
            Self::TxQueueFull => write!(f, "transmit queue full"),
            Self::Protocol(s) => write!(f, "protocol error: {s}"),
            Self::Fatal(s) => write!(f, "fatal error: {s}"),
            Self::Disconnected => write!(f, "USB device disconnected"),
        }
    }
}

/// Selects which adapter backend to use when opening a session.
///
/// Each variant carries **all identity fields needed to open the adapter**.
/// The top-level `SessionConfig::port` field is a legacy alias for Summit
/// channel and SocketCAN interface name; it will be retired in favour of
/// variant-local fields (Issue A in the action plan).
///
/// ## Exclusivity
///
/// A given physical adapter may be owned by at most one running RustyCAN
/// instance at a time.  For USB variants this is enforced by the OS (nusb
/// exclusive claim).  For KCanNet it is enforced by kgate's 1-to-1 room
/// pairing.  For SocketCAN an application-level PID lockfile is planned.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum AdapterKind {
    /// Summit PCAN-USB dongle accessed via `host-can` / libPCBUSB.
    ///
    /// macOS and Windows only.  On Linux, Summit hardware appears as a
    /// SocketCAN interface via the `peak_usb` kernel driver — use `SocketCan`
    /// on Linux instead.
    ///
    /// `port` (top-level `SessionConfig` field) is the channel number:
    /// `"1"` for PCAN_USBBUS1, etc.  This will move into the variant
    /// as `Summit { channel: String }` (Issue A).
    Summit,
    /// KCAN dongle connected over USB.  Hardware timestamp source: FDCAN RXTS
    /// latched at CAN frame SOF (100 ns, `hardware_timestamp_ns: Some`).
    ///
    /// `serial` optionally pins a specific dongle by its USB serial string.
    /// When `None`, the first KCAN device found is used.
    KCan { serial: Option<String> },
    /// Linux kernel SocketCAN interface (e.g. `can0` from the `peak_usb` or
    /// `gs_usb` driver).  Host-approximate timestamps (`hardware_timestamp_ns:
    /// None`).
    ///
    /// `port` (top-level `SessionConfig` field) holds the interface name
    /// (`"can0"`, `"can1"`, …).  This will move into the variant as
    /// `SocketCan { iface: String }` (Issue A).
    ///
    /// On Linux, Summit and Apex hardware may appear here via their kernel
    /// drivers.  The connect UI merges those into their respective adapter
    /// entries to avoid showing the same device twice.
    SocketCan,
    /// Apex USB-CAN device, cross-platform userspace USB driver (nusb).
    /// Host-approximate timestamps (`hardware_timestamp_ns: None`).
    ///
    /// On Linux, if the kernel has bound a driver (e.g. `gs_usb`) to the Apex
    /// device, nusb cannot claim it.  Use `SocketCan` in that case.
    /// `apex::find_socketcan_interface()` detects which path applies.
    ///
    /// `serial` optionally pins a specific module by its USB serial string.
    /// When `None`, the first Apex device found is used.
    Apex { serial: Option<String> },
    /// KCAN-over-TCP with X25519 + AES-256-GCM session encryption.
    /// Hardware timestamp source: same FDCAN RXTS as KCan USB — frozen in
    /// firmware before entering the TCP stack (`hardware_timestamp_ns: Some`).
    ///
    /// `uri` is the `K1:<8-hex-ip>/<43-base64url-pubkey>` or
    /// `K1:r/<6-char-room-id>/<43-base64url-pubkey>` string scanned from
    /// the device's e-paper QR code (ADR-0017 in rustyepd).
    KCanNet { uri: String },
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

    /// Human-readable adapter name for log messages, UI display, and the
    /// `SESSION_START` JSONL event (planned, Issue C).
    fn name(&self) -> &str;

    /// Firmware version reported by the device during open, if available.
    ///
    /// Returns `Some((major, minor, patch))` for KCAN dongles; `None` for
    /// all other adapters (Summit, virtual, etc.).
    fn firmware_version(&self) -> Option<(u8, u8, u8)> {
        None
    }

    /// Whether the adapter reports its own transmitted frames back through
    /// [`Self::recv`] as TX echoes (`ReceivedFrame::is_tx_echo == true`).
    ///
    /// KCAN dongles echo TX; Summit and SocketCAN do not. Callers use this to
    /// decide whether a host-initiated transmit will re-enter the receive path
    /// (and thus be surfaced to the live sniffer there) or must be reported at
    /// the send site instead.
    fn echoes_tx(&self) -> bool {
        false
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
        AdapterKind::Summit => {
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
                            "Summit driver library not found. \
                            Please install the PCANBasic driver:\n\
                            • macOS: https://mac-can.com\n\
                            • Windows: https://peak-system.com/downloads\n\
                            ({detail})"
                        ))
                    } else {
                        AdapterError::NotFound(detail)
                    }
                })?;
                Ok(Box::new(summit::SummitAdapter::new(inner)))
            }
            #[cfg(target_os = "linux")]
            Err(AdapterError::NotFound(
                "Summit is not supported on Linux via the proprietary driver. \
                Use the KCAN dongle instead, or connect via SocketCAN."
                    .into(),
            ))
        }
        AdapterKind::KCan { serial } => {
            let adapter = kcan::KCanAdapter::open(serial.as_deref(), baud, listen_only)?;
            Ok(Box::new(adapter))
        }
        AdapterKind::SocketCan => {
            #[cfg(target_os = "linux")]
            {
                let adapter = socketcan_adapter::SocketCanAdapter::open(port)?;
                Ok(Box::new(adapter))
            }
            #[cfg(not(target_os = "linux"))]
            Err(AdapterError::NotFound(
                "SocketCAN is only available on Linux.".into(),
            ))
        }
        AdapterKind::Apex { serial } => {
            // On Linux, prefer a kernel driver: if a SocketCAN interface backed
            // by the Apex device exists, use it; otherwise fall back to the
            // cross-platform nusb userspace driver.
            #[cfg(target_os = "linux")]
            if let Some(iface) = apex::find_socketcan_interface(serial.as_deref()) {
                let adapter = socketcan_adapter::SocketCanAdapter::open(&iface)?;
                return Ok(Box::new(adapter));
            }
            let adapter = apex::ApexAdapter::open(serial.as_deref(), baud, listen_only)?;
            Ok(Box::new(adapter))
        }
        AdapterKind::KCanNet { uri } => {
            let adapter = kcan_net::KCanNetAdapter::open(uri)?;
            Ok(Box::new(adapter))
        }
    }
}

/// Probe whether an adapter is reachable without starting a session.
///
/// Used by the Connect-screen polling loop.
///
/// # Summit probing strategy
///
/// Do NOT use `host_can::adapter::get_adapter()` to probe for Summit hardware.
/// That function opens the PCAN channel (CAN_Initialize) and then immediately
/// drops it (CAN_Uninitialize + dlclose).  The macOS Summit driver
/// (libPCBUSB.dylib) starts internal USB callback threads on CAN_Initialize;
/// dlclose frees the library's text segment while those threads are still
/// running, producing a SIGSEGV on the next open.
///
/// Instead, detect Summit hardware by scanning USB devices for Summit System's
/// vendor ID (0x0C72), which is safe to call repeatedly from any thread.
pub fn probe_adapter_kind(kind: &AdapterKind, _port: &str, _baud: u32) -> bool {
    match kind {
        AdapterKind::Summit => {
            #[cfg(target_os = "macos")]
            {
                // nusb::list_devices() on macOS 26 Tahoe triggers a stack
                // overflow in libusb's IOKit CFRunLoop thread when a Summit
                // adapter is present (libusb + Tahoe + Summit USB interaction
                // bug).  Use ioreg via subprocess to avoid touching the USB
                // device directly from this process.
                std::process::Command::new("ioreg")
                    .args(["-p", "IOUSB", "-l", "-w0"])
                    .output()
                    .map(|out| {
                        let stdout = String::from_utf8_lossy(&out.stdout);
                        // ioreg prints idVendor as a decimal integer.
                        // Summit System VID = 0x0C72 = 3186 decimal.
                        stdout.contains("\"idVendor\" = 3186")
                    })
                    .unwrap_or(false)
            }
            #[cfg(target_os = "windows")]
            {
                // On Windows use nusb to enumerate USB devices and check for
                // the Summit System vendor ID (0x0C72 = 3186).  The macOS Tahoe
                // nusb stack-overflow bug does not affect Windows.
                use nusb::MaybeFuture as _;
                const SUMMIT_VID: u16 = 0x0C72;
                nusb::list_devices()
                    .wait()
                    .map(|mut iter| iter.any(|d| d.vendor_id() == SUMMIT_VID))
                    .unwrap_or(false)
            }
            #[cfg(target_os = "linux")]
            false
        }
        AdapterKind::KCan { serial } => kcan::KCanAdapter::probe(serial.as_deref()),
        AdapterKind::SocketCan => {
            #[cfg(target_os = "linux")]
            {
                socketcan_adapter::SocketCanAdapter::probe(_port)
            }
            #[cfg(not(target_os = "linux"))]
            false
        }
        AdapterKind::Apex { serial } => {
            // Present if either a kernel-driver SocketCAN interface (Linux) or
            // the raw USB device is available.
            #[cfg(target_os = "linux")]
            if apex::find_socketcan_interface(serial.as_deref()).is_some() {
                return true;
            }
            apex::ApexAdapter::probe(serial.as_deref())
        }
        // Probe by attempting a short TCP connect to the IP encoded in the URI.
        AdapterKind::KCanNet { uri } => kcan_net::KCanNetAdapter::probe(uri),
    }
}
