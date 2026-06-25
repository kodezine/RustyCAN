//! SocketCAN adapter — connects to a Linux kernel CAN network interface
//! (e.g. `can0` provided by the `peak_usb` driver) without any proprietary library.
//!
//! The adapter opens a raw `PF_CAN` socket bound to the named interface.
//! All CAN 2.0 data and RTR frames on the bus are received; the kernel deduplicates
//! any TX echo if `set_recv_own_msgs(false)` is respected by the driver.
//!
//! # Quickstart
//!
//! Bring up the interface (once per boot or after plug):
//! ```sh
//! sudo ip link set can0 up type can bitrate 250000
//! ```
//!
//! Then set `"adapter_kind": "SocketCan"` and `"port": "can0"` in your config.

use std::io::ErrorKind;
use std::time::Duration;

use embedded_can::Frame as EmbeddedFrame;
use host_can::frame::CanFrame;
use socketcan::{CanDataFrame, CanRemoteFrame, CanSocket, Socket, SocketOptions};

use super::{AdapterError, CanAdapter, ReceivedFrame};

pub struct SocketCanAdapter {
    socket: CanSocket,
    name: String,
}

impl SocketCanAdapter {
    /// Open the named SocketCAN interface (e.g. `"can0"`).
    ///
    /// Performs a pre-flight sysfs check before opening the socket and returns
    /// a step-by-step [`AdapterError::NotFound`] message for every common
    /// setup problem a first-time Linux user might encounter:
    ///
    /// | Situation | Guidance shown |
    /// |-----------|---------------|
    /// | `peak_usb` module not loaded | `sudo modprobe peak_usb` + bring up |
    /// | Module loaded, interface missing | lists available CAN interfaces |
    /// | Interface exists but is DOWN | `sudo ip link set <iface> up …` |
    /// | Interface is not a CAN device | points to `ip link show` |
    pub fn open(interface: &str) -> Result<Self, AdapterError> {
        // ── Pre-flight diagnostics ────────────────────────────────────────
        let sys_iface = format!("/sys/class/net/{interface}");

        if !std::path::Path::new(&sys_iface).exists() {
            // Interface is absent from the kernel.
            // Check whether the peak_usb (or any other CAN) module is loaded.
            let peak_usb_loaded = std::path::Path::new("/sys/module/peak_usb").exists();

            if !peak_usb_loaded {
                return Err(AdapterError::NotFound(format!(
                    "SocketCAN interface '{interface}' not found and the peak_usb kernel\n\
                     module is not loaded.\n\n\
                     Step 1 — load the driver (once per boot):\n\
                     \tsudo modprobe peak_usb\n\n\
                     Step 2 — plug in the PEAK adapter (or re-plug if already connected).\n\n\
                     Step 3 — bring up the interface:\n\
                     \tsudo ip link set {interface} up type can bitrate 250000\n\n\
                     Verify:  ip link show | grep can"
                )));
            }

            // Module is loaded but the named interface is missing.
            // The adapter may be on a different port or under a different name.
            let hint = match Self::list_can_interfaces() {
                Some(ref v) if !v.is_empty() => {
                    format!("\n\nAvailable CAN interfaces: {}", v.join(", "))
                }
                _ => "\n\n(No CAN interfaces are currently up — is the adapter plugged in?)".into(),
            };

            return Err(AdapterError::NotFound(format!(
                "SocketCAN interface '{interface}' not found.{hint}\n\n\
                 Bring up the correct interface:\n\
                 \tsudo ip link set <iface> up type can bitrate 250000"
            )));
        }

        // Interface exists — verify it is actually a CAN interface (ARPHRD_CAN = 280).
        let iface_type = std::fs::read_to_string(format!("{sys_iface}/type")).unwrap_or_default();
        if iface_type.trim() != "280" {
            return Err(AdapterError::NotFound(format!(
                "'{interface}' is not a CAN interface (type={}).\n\
                 Check available CAN interfaces: ip link show | grep can",
                iface_type.trim()
            )));
        }

        // Check IFF_UP: CanSocket::open() succeeds on a DOWN interface, but
        // recv() would immediately return ENETDOWN.  Fail here instead with a
        // clear "bring it up" message.
        let flags_raw = std::fs::read_to_string(format!("{sys_iface}/flags")).unwrap_or_default();
        let flags = u32::from_str_radix(flags_raw.trim().trim_start_matches("0x"), 16).unwrap_or(0);
        const IFF_UP: u32 = 0x1;
        if flags & IFF_UP == 0 {
            return Err(AdapterError::NotFound(format!(
                "SocketCAN interface '{interface}' exists but is DOWN.\n\n\
                 Bring it up:\n\
                 \tsudo ip link set {interface} up type can bitrate 250000"
            )));
        }

        // ── Open the socket ───────────────────────────────────────────────
        let socket = CanSocket::open(interface).map_err(|e| AdapterError::Io(e.to_string()))?;

        // Do not receive echoes of frames we sent ourselves.
        socket
            .set_recv_own_msgs(false)
            .map_err(|e| AdapterError::Io(e.to_string()))?;

        let name = format!("SocketCAN ({interface})");
        Ok(Self { socket, name })
    }

    /// List all SocketCAN interfaces currently present on the system.
    ///
    /// Walks `/sys/class/net/` and returns names of entries whose `type` file
    /// reads `280` (ARPHRD_CAN).  Returns `None` if the directory is not
    /// accessible (shouldn't happen on a running Linux kernel).
    fn list_can_interfaces() -> Option<Vec<String>> {
        let mut ifaces: Vec<String> = std::fs::read_dir("/sys/class/net")
            .ok()?
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let type_file = entry.path().join("type");
                std::fs::read_to_string(type_file)
                    .ok()
                    .filter(|t| t.trim() == "280")
                    .map(|_| name)
            })
            .collect();
        ifaces.sort();
        Some(ifaces)
    }

    /// Return `true` when the named interface exists and is a CAN interface.
    ///
    /// Checks `/sys/class/net/<iface>/type` for `280` (ARPHRD_CAN).  The
    /// interface may be DOWN; `open()` will succeed, but `recv()` will return
    /// `Disconnected` until it is brought up.
    pub fn probe(interface: &str) -> bool {
        std::fs::read_to_string(format!("/sys/class/net/{interface}/type"))
            .map(|s| s.trim() == "280")
            .unwrap_or(false)
    }
}

impl CanAdapter for SocketCanAdapter {
    fn recv(&mut self, timeout: Duration) -> Result<ReceivedFrame, AdapterError> {
        match self.socket.read_frame_timeout(timeout) {
            Ok(socketcan::CanFrame::Data(df)) => {
                let frame = CanFrame::new(df.id(), df.data())
                    .ok_or_else(|| AdapterError::Io("malformed SocketCAN data frame".into()))?;
                Ok(ReceivedFrame {
                    frame,
                    hardware_timestamp_ns: None,
                    channel: 0,
                    is_tx_echo: false,
                })
            }

            Ok(socketcan::CanFrame::Remote(rf)) => {
                let frame = CanFrame::new_remote(rf.id(), rf.dlc())
                    .ok_or_else(|| AdapterError::Io("malformed SocketCAN RTR frame".into()))?;
                Ok(ReceivedFrame {
                    frame,
                    hardware_timestamp_ns: None,
                    channel: 0,
                    is_tx_echo: false,
                })
            }

            // Error frames (bus-off, etc.) — signal disconnect so the session
            // enters the reconnect loop rather than spinning on error frames.
            Ok(socketcan::CanFrame::Error(_)) => Err(AdapterError::Disconnected),

            // Clean receive-timeout (normal polling interval with no traffic).
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                Err(AdapterError::Timeout)
            }

            // ENETDOWN — interface was taken down while the session was running.
            Err(e) if e.raw_os_error() == Some(100) => Err(AdapterError::Disconnected),

            Err(e) => Err(AdapterError::Io(e.to_string())),
        }
    }

    fn send(&mut self, frame: &CanFrame) -> Result<(), AdapterError> {
        if frame.is_remote_frame() {
            let rf = CanRemoteFrame::new_remote(frame.id(), frame.dlc())
                .ok_or_else(|| AdapterError::Io("could not build SocketCAN RTR frame".into()))?;
            self.socket
                .write_frame_insist(&rf)
                .map_err(|e| AdapterError::Io(e.to_string()))
        } else {
            let df = CanDataFrame::new(frame.id(), frame.data())
                .ok_or_else(|| AdapterError::Io("could not build SocketCAN data frame".into()))?;
            self.socket
                .write_frame_insist(&df)
                .map_err(|e| AdapterError::Io(e.to_string()))
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}
