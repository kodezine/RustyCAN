//! Apex USB-CAN adapter — pure-Rust userspace USB via `nusb`.
//!
//! Cross-platform (macOS/Linux/Windows) driver modeled on the KCAN dongle
//! adapter ([`super::kcan`]).  A single `nusb`-based code path talks to the
//! device directly — no vendor DLL, no SocketCAN, no proprietary library.
//!
//! # Two device states (issue #103)
//!
//! The Apex USB-CAN device powers up as a **bootloader** (single loader endpoint).
//! [`ApexAdapter::open`] issues the vendor RECONNECT request so the
//! bootloader boots the flashed application firmware; the device then
//! re-enumerates in **running mode** exposing five endpoints (DATA in/out,
//! MSG in/out, STAT in).  A background thread carries CAN frames while the
//! session thread talks to it over `mpsc` channels, like the KCAN adapter.
//!
//! The wire protocol ([`protocol`]) is a clean-room MIT implementation decoded
//! from bench USB captures, not from any GPL driver source.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use embedded_can::{ExtendedId, Frame as EmbeddedFrame, Id, StandardId};
use nusb::transfer::{
    Buffer, Bulk, ControlIn, ControlOut, ControlType, In, Interrupt, Out, Recipient,
};
use nusb::{DeviceInfo, Endpoint, MaybeFuture};

use host_can::frame::CanFrame;

use super::{AdapterError, CanAdapter, ReceivedFrame};

/// Clean-room USB wire protocol (frame + command codec), decoded from bench
/// captures — see issue #103.
mod protocol;

/// USB vendor ID of the Apex USB-CAN hardware (confirmed from hardware descriptor).
const APEX_VID: u16 = 0x0878;

/// `None` matches any product under the Apex vendor ID so the whole
/// Apex USB-CAN device family (bootloader and running PIDs) is recognized.
const APEX_PID: Option<u16> = None;

/// Interface claimed in both bootloader and running mode.
const IFACE_NUM: u8 = 0;
const CTRL_TIMEOUT: Duration = Duration::from_millis(500);
const CMD_TIMEOUT: Duration = Duration::from_millis(500);
/// How long to wait for the device to re-enumerate in running mode after the
/// bootloader RECONNECT request.
const RECONNECT_WAIT: Duration = Duration::from_secs(6);

/// TX command queued from the session thread to the reader thread.
enum TxCmd {
    Send([u8; protocol::FRAME_SIZE]),
    Shutdown,
}

/// Apex USB-CAN adapter (running mode).
pub struct ApexAdapter {
    frame_rx: mpsc::Receiver<protocol::ApexFrame>,
    error_rx: mpsc::Receiver<String>,
    tx_cmd_tx: mpsc::SyncSender<TxCmd>,
    reader_thread: Option<thread::JoinHandle<()>>,
    name: String,
}

impl ApexAdapter {
    /// Open the Apex device (optionally pinned by USB serial), boot it into
    /// running mode if necessary, program the bitrate, go bus-on, and spawn the
    /// reader thread.
    pub fn open(serial: Option<&str>, baud: u32, listen_only: bool) -> Result<Self, AdapterError> {
        let baud_ex = protocol::baud_ex_reg(baud).ok_or_else(|| {
            AdapterError::NotFound(format!(
                "Apex: unsupported bitrate {baud}; use a standard CAN rate \
                 (10000, 20000, 50000, 100000, 125000, 250000, 500000, 800000, 1000000)"
            ))
        })?;

        // Boot from bootloader to application firmware if needed.
        let mut info = find_device_info(serial)?;
        if !protocol::PID_RUNNING.contains(&info.product_id()) {
            reconnect_from_bootloader(&info)?;
            info = wait_for_running(serial, RECONNECT_WAIT)?;
        }
        let name = describe(&info);

        let device = info
            .open()
            .wait()
            .map_err(|e| AdapterError::Io(format!("open device: {e}")))?;
        let iface = claim_iface0(&device)?;

        // Command endpoints, then the bus-on handshake.
        let mut msg_out = iface
            .endpoint::<Bulk, Out>(protocol::EP_MSG_OUT)
            .map_err(|e| AdapterError::Io(format!("open msg-out ep: {e}")))?;
        let mut msg_in = iface
            .endpoint::<Bulk, In>(protocol::EP_MSG_IN)
            .map_err(|e| AdapterError::Io(format!("open msg-in ep: {e}")))?;

        let mode = protocol::MODE_NORMAL
            | if listen_only {
                protocol::MODE_LISTEN_ONLY
            } else {
                0
            };
        for cmd in protocol::open_sequence(0, baud_ex, mode) {
            send_cmd(&mut msg_out, &mut msg_in, &cmd)?;
        }

        // Data + status endpoints move into the reader thread.
        let data_out = iface
            .endpoint::<Bulk, Out>(protocol::EP_DATA_OUT)
            .map_err(|e| AdapterError::Io(format!("open data-out ep: {e}")))?;
        let data_in = iface
            .endpoint::<Bulk, In>(protocol::EP_DATA_IN)
            .map_err(|e| AdapterError::Io(format!("open data-in ep: {e}")))?;
        let stat_in = iface
            .endpoint::<Interrupt, In>(protocol::EP_STAT_IN)
            .map_err(|e| AdapterError::Io(format!("open stat-in ep: {e}")))?;

        let (frame_tx, frame_rx) = mpsc::channel();
        let (tx_cmd_tx, tx_cmd_rx) = mpsc::sync_channel::<TxCmd>(32);
        let (error_tx, error_rx) = mpsc::sync_channel::<String>(1);

        let reader_thread = thread::Builder::new()
            .name("apex-reader".into())
            .spawn(move || {
                reader_thread(
                    data_in, stat_in, data_out, msg_out, frame_tx, tx_cmd_rx, error_tx,
                )
            })
            .map_err(|e| AdapterError::Io(format!("spawn reader: {e}")))?;

        // The reader thread's endpoints keep the interface claim alive, so
        // `iface` itself need not be retained here.
        drop(iface);

        Ok(Self {
            frame_rx,
            error_rx,
            tx_cmd_tx,
            reader_thread: Some(reader_thread),
            name,
        })
    }

    /// Return `true` while a Apex device (optionally matching `serial`) is
    /// enumerated on USB.
    pub fn probe(serial: Option<&str>) -> bool {
        find_device_info(serial).is_ok()
    }

    /// List connected Apex devices as `(serial, display_name)` pairs for the
    /// Connect-screen device picker.
    pub fn list_devices() -> Vec<(String, String)> {
        let Ok(iter) = nusb::list_devices().wait() else {
            return vec![];
        };
        iter.filter(is_apex)
            .map(|d| {
                let serial = d.serial_number().unwrap_or("").to_string();
                (serial, describe(&d))
            })
            .collect()
    }
}

impl CanAdapter for ApexAdapter {
    fn recv(&mut self, timeout: Duration) -> Result<ReceivedFrame, AdapterError> {
        loop {
            match self.frame_rx.recv_timeout(timeout) {
                Ok(f) => {
                    // Skip frames with a malformed ID rather than aborting.
                    let Some(frame) = apex_to_can_frame(&f) else {
                        continue;
                    };
                    return Ok(ReceivedFrame {
                        frame,
                        hardware_timestamp_ns: None,
                        channel: 0,
                        is_tx_echo: false,
                    });
                }
                Err(mpsc::RecvTimeoutError::Timeout) => return Err(AdapterError::Timeout),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let reason = self.error_rx.try_recv().unwrap_or_default();
                    if !reason.is_empty() && reason != "__disconnected__" {
                        eprintln!("Apex reader thread died: {reason} (treating as disconnect)");
                    }
                    return Err(AdapterError::Disconnected);
                }
            }
        }
    }

    fn send(&mut self, frame: &CanFrame) -> Result<(), AdapterError> {
        self.tx_cmd_tx
            .try_send(TxCmd::Send(can_frame_to_bytes(frame)))
            .map_err(|_| AdapterError::Io("Apex TX queue full".into()))
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for ApexAdapter {
    fn drop(&mut self) {
        // Ask the reader thread to send SHUTDOWN + RESET and exit; join so its
        // endpoints are released (freeing the interface claim) before we return.
        let _ = self.tx_cmd_tx.try_send(TxCmd::Shutdown);
        if let Some(handle) = self.reader_thread.take() {
            let _ = handle.join();
        }
    }
}

// ─── Background IO thread ─────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn reader_thread(
    mut data_in: Endpoint<Bulk, In>,
    mut stat_in: Endpoint<Interrupt, In>,
    mut data_out: Endpoint<Bulk, Out>,
    mut msg_out: Endpoint<Bulk, Out>,
    frame_tx: mpsc::Sender<protocol::ApexFrame>,
    tx_cmd_rx: mpsc::Receiver<TxCmd>,
    error_tx: mpsc::SyncSender<String>,
) {
    // 64 = full-speed bulk MPS; also a multiple of the status EP packet size.
    const BUF: usize = 64;
    let mut acc: Vec<u8> = Vec::with_capacity(BUF * 2);

    loop {
        // 1. Drain queued TX / shutdown requests.
        loop {
            match tx_cmd_rx.try_recv() {
                Ok(TxCmd::Send(bytes)) => {
                    let _ = data_out
                        .transfer_blocking(bytes.to_vec().into(), Duration::from_millis(100));
                }
                Ok(TxCmd::Shutdown) => {
                    let _ = msg_out.transfer_blocking(
                        protocol::cmd_shutdown(0).to_vec().into(),
                        Duration::from_millis(100),
                    );
                    let _ = msg_out.transfer_blocking(
                        protocol::cmd_reset(0).to_vec().into(),
                        Duration::from_millis(100),
                    );
                    return;
                }
                Err(_) => break,
            }
        }

        // 2. Keep a status (interrupt IN) read outstanding — the device
        //    disconnects if the host stops polling status.  Content is ignored.
        let _ = stat_in.transfer_blocking(Buffer::new(BUF), Duration::from_millis(5));

        // 3. Read CAN frames (bulk IN), accumulating 16-byte records.
        match data_in
            .transfer_blocking(Buffer::new(BUF), Duration::from_millis(20))
            .into_result()
        {
            Ok(data) if !data.is_empty() => {
                acc.extend_from_slice(&data);
                while acc.len() >= protocol::FRAME_SIZE {
                    let mut rec = [0u8; protocol::FRAME_SIZE];
                    rec.copy_from_slice(&acc[..protocol::FRAME_SIZE]);
                    acc.drain(..protocol::FRAME_SIZE);
                    if let protocol::DataRecord::Frame(f) = protocol::decode_frame(&rec) {
                        if frame_tx.send(f).is_err() {
                            return; // adapter dropped
                        }
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                use nusb::transfer::TransferError;
                match e {
                    TransferError::Disconnected => {
                        let _ = error_tx.try_send("__disconnected__".into());
                        return;
                    }
                    TransferError::Stall => {
                        if data_in.clear_halt().wait().is_err() {
                            let _ = error_tx.try_send("bulk-in stall".into());
                            return;
                        }
                        acc.clear();
                    }
                    // Timeout / Cancelled / InvalidArgument are transient on a
                    // quiet bus; keep polling.
                    _ => {}
                }
            }
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Send one 8-byte command on MSG_OUT and read its reply on MSG_IN.
fn send_cmd(
    out: &mut Endpoint<Bulk, Out>,
    inp: &mut Endpoint<Bulk, In>,
    cmd: &[u8; protocol::CMD_SIZE],
) -> Result<(), AdapterError> {
    out.transfer_blocking(cmd.to_vec().into(), CMD_TIMEOUT)
        .into_result()
        .map_err(|e| AdapterError::Protocol(format!("cmd 0x{:02x} send: {e:?}", cmd[0])))?;
    // Reply buffer must be a multiple of the endpoint packet size (64).
    inp.transfer_blocking(Buffer::new(64), CMD_TIMEOUT)
        .into_result()
        .map_err(|e| AdapterError::Protocol(format!("cmd 0x{:02x} reply: {e:?}", cmd[0])))?;
    Ok(())
}

/// Claim interface 0, selecting configuration 1 first if the device is
/// unconfigured.
///
/// macOS leaves this vendor-class device unconfigured, so no interface service
/// nodes exist and a direct claim fails with "interface not found".  We only
/// fall back to `set_configuration` when the direct claim fails, to avoid the
/// spurious device reset it can trigger on an already-configured device.
fn claim_iface0(device: &nusb::Device) -> Result<nusb::Interface, AdapterError> {
    match device.claim_interface(IFACE_NUM).wait() {
        Ok(iface) => Ok(iface),
        Err(_) => {
            let _ = device.set_configuration(1).wait();
            device
                .claim_interface(IFACE_NUM)
                .wait()
                .map_err(|e| AdapterError::Io(format!("claim interface {IFACE_NUM}: {e}")))
        }
    }
}

/// Tell a bootloader-mode device to boot its flashed application firmware.
fn reconnect_from_bootloader(info: &DeviceInfo) -> Result<(), AdapterError> {
    let device = info
        .open()
        .wait()
        .map_err(|e| AdapterError::Io(format!("open bootloader: {e}")))?;
    let iface = claim_iface0(&device)?;
    // Boot-arm handshake for the 0x1122-generation bootloader (observed via
    // usbmon): it must be told the application flash address (SET_BOOT_ADDR)
    // before RECONNECT will jump to it.  Bootloaders that boot on RECONNECT
    // alone are unaffected, so these steps are best-effort and errors ignored.
    for req in [protocol::VRREQ_READ_VERSION, protocol::VRREQ_READ_HWINFO] {
        let _ = iface
            .control_in(
                ControlIn {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: req,
                    value: 0,
                    index: 0,
                    length: 4,
                },
                CTRL_TIMEOUT,
            )
            .wait();
    }
    let _ = iface
        .control_out(
            ControlOut {
                control_type: ControlType::Vendor,
                recipient: Recipient::Device,
                request: protocol::VRREQ_SET_BOOT_ADDR,
                value: 0,
                index: 0,
                data: &protocol::BOOT_ADDR_ARM,
            },
            CTRL_TIMEOUT,
        )
        .wait();
    let _ = iface
        .control_in(
            ControlIn {
                control_type: ControlType::Vendor,
                recipient: Recipient::Device,
                request: protocol::VRREQ_BOOT_STATUS,
                value: 0,
                index: 0,
                length: 12,
            },
            CTRL_TIMEOUT,
        )
        .wait();
    // VRREQ RECONNECT (0xB6): vendor OUT, no data — the device jumps to the
    // application and re-enumerates, so it may drop before ACKing.
    let _ = iface
        .control_out(
            ControlOut {
                control_type: ControlType::Vendor,
                recipient: Recipient::Device,
                request: protocol::VRREQ_RECONNECT,
                value: 0,
                index: 0,
                data: &[],
            },
            CTRL_TIMEOUT,
        )
        .wait();
    Ok(())
}

/// Poll USB enumeration until a running-mode Apex device appears.
fn wait_for_running(serial: Option<&str>, timeout: Duration) -> Result<DeviceInfo, AdapterError> {
    let start = Instant::now();
    loop {
        if let Ok(iter) = nusb::list_devices().wait() {
            for info in iter {
                if info.vendor_id() == APEX_VID
                    && protocol::PID_RUNNING.contains(&info.product_id())
                    && serial.is_none_or(|s| info.serial_number().unwrap_or("") == s)
                {
                    return Ok(info);
                }
            }
        }
        if start.elapsed() > timeout {
            return Err(AdapterError::NotFound(
                "Apex device did not re-enumerate in running mode after RECONNECT".into(),
            ));
        }
        thread::sleep(Duration::from_millis(150));
    }
}

fn apex_to_can_frame(f: &protocol::ApexFrame) -> Option<CanFrame> {
    let id = if f.extended {
        Id::Extended(ExtendedId::new(f.id & 0x1FFF_FFFF)?)
    } else {
        Id::Standard(StandardId::new((f.id & 0x7FF) as u16)?)
    };
    let n = (f.dlc as usize).min(8);
    if f.rtr {
        CanFrame::new_remote(id, n)
    } else {
        CanFrame::new(id, &f.data[..n])
    }
}

fn can_frame_to_bytes(frame: &CanFrame) -> [u8; protocol::FRAME_SIZE] {
    let (id, extended) = match frame.id() {
        Id::Standard(s) => (s.as_raw() as u32, false),
        Id::Extended(e) => (e.as_raw(), true),
    };
    protocol::encode_frame(
        id,
        extended,
        frame.is_remote_frame(),
        frame.dlc() as u8,
        frame.data(),
    )
}

/// Whether a USB device is a Apex device we can drive.
fn is_apex(d: &DeviceInfo) -> bool {
    d.vendor_id() == APEX_VID && APEX_PID.is_none_or(|pid| d.product_id() == pid)
}

/// Neutral `"Apex [VID:PID]"` identifier for logs and the picker.
///
/// Deliberately ignores the device's own USB product string so no vendor
/// branding from the hardware descriptor leaks into the UI.
fn describe(d: &DeviceInfo) -> String {
    format!("Apex [{:04X}:{:04X}]", d.vendor_id(), d.product_id())
}

fn find_device_info(serial: Option<&str>) -> Result<DeviceInfo, AdapterError> {
    let iter = nusb::list_devices()
        .wait()
        .map_err(|e| AdapterError::Io(format!("USB enumeration: {e}")))?;
    for info in iter {
        if !is_apex(&info) {
            continue;
        }
        if let Some(s) = serial {
            if info.serial_number().unwrap_or("") != s {
                continue;
            }
        }
        return Ok(info);
    }
    Err(AdapterError::NotFound(match serial {
        Some(s) => format!("Apex device serial '{s}' not found"),
        None => format!("no Apex device found (VID=0x{APEX_VID:04X})"),
    }))
}

/// Find a SocketCAN interface (`canX`) backed by an Apex USB device.
///
/// Returns the interface name when a kernel CAN driver is bound to the device
/// (optionally matching `serial`), so the caller can drive it via SocketCAN
/// instead of the userspace USB driver.  `None` means no kernel-driver
/// interface exists and the nusb path should be used.
#[cfg(target_os = "linux")]
pub(super) fn find_socketcan_interface(serial: Option<&str>) -> Option<String> {
    use std::path::Path;

    // Read the USB vendor ID and serial of the device backing a netdev whose
    // `device` symlink points at a USB interface (…:1.0); its parent is the
    // USB device directory holding `idVendor` / `serial`.
    fn usb_ids(iface_link: &Path) -> Option<(u16, Option<String>)> {
        let usb_iface = std::fs::canonicalize(iface_link).ok()?;
        let dev_dir = usb_iface.parent()?;
        let vid = u16::from_str_radix(
            std::fs::read_to_string(dev_dir.join("idVendor"))
                .ok()?
                .trim(),
            16,
        )
        .ok()?;
        let sn = std::fs::read_to_string(dev_dir.join("serial"))
            .ok()
            .map(|s| s.trim().to_string());
        Some((vid, sn))
    }

    for entry in std::fs::read_dir("/sys/class/net").ok()?.flatten() {
        let base = entry.path();
        // ARPHRD_CAN = 280 marks a CAN interface.
        let is_can = std::fs::read_to_string(base.join("type"))
            .map(|s| s.trim() == "280")
            .unwrap_or(false);
        if !is_can {
            continue;
        }
        match usb_ids(&base.join("device")) {
            Some((APEX_VID, sn)) if serial.is_none_or(|s| sn.as_deref() == Some(s)) => {
                return Some(entry.file_name().to_string_lossy().into_owned());
            }
            _ => {}
        }
    }
    None
}
