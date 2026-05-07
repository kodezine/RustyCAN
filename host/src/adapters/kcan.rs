//! KCAN dongle adapter — pure-Rust USB via `nusb`.
//!
//! A background reader thread owns `nusb::Interface` and handles both
//! bulk IN (RX frames) and bulk OUT (TX frames).  The session thread
//! communicates via two `std::sync::mpsc` channels.

use std::sync::mpsc;
use std::time::Duration;

use nusb::transfer::{Buffer, Bulk, ControlIn, ControlOut, ControlType, In, Out, Recipient};
use nusb::{DeviceInfo, Endpoint, MaybeFuture};

use host_can::frame::CanFrame;

use kcan_protocol::control::{KCanBitTiming, KCanBtConst, KCanDeviceInfo, KCanMode, RequestCode};
use kcan_protocol::frame::{FrameFlags, FrameType, KCanFrame, KCAN_FRAME_SIZE};

use super::{AdapterError, CanAdapter, ReceivedFrame};

const KCAN_VID: u16 = 0x1209;
const KCAN_PID: u16 = 0xBEEF;
const BULK_IN_EP: u8 = 0x81; // device→host  (Embassy allocates as 0x81)
const BULK_OUT_EP: u8 = 0x01; // host→device  (Embassy allocates as 0x01, not 0x02)
const CTRL_TIMEOUT: Duration = Duration::from_millis(500);

enum TxCmd {
    Send(Vec<u8>),
    Shutdown,
}

pub struct KCanAdapter {
    /// Kept alive so `Drop` can send SET_MODE(bus_off) without re-opening.
    iface: nusb::Interface,
    frame_rx: mpsc::Receiver<KCanFrame>,
    /// One-shot channel: the reader_thread sends the reason it died before exiting.
    error_rx: mpsc::Receiver<String>,
    tx_cmd_tx: mpsc::SyncSender<TxCmd>,
    /// Joined in `Drop` to ensure ep_in/ep_out are released before the OS
    /// interface claim is freed, preventing claim_interface() failures on reconnect.
    reader_thread: Option<std::thread::JoinHandle<()>>,
    pub fw_version: (u8, u8, u8),
    name: String,
    tx_seq: u16,
}

impl KCanAdapter {
    pub fn open(serial: Option<&str>, baud: u32, listen_only: bool) -> Result<Self, AdapterError> {
        let dev_info = find_device_info(serial)?;
        let device = dev_info
            .open()
            .wait()
            .map_err(|e| AdapterError::Io(format!("open device: {e}")))?;

        // On macOS (IOUSBHostFamily), claim_interface() internally calls
        // USBInterfaceOpen with kUSBOptionBitOpenExclusivelyMask, giving us
        // exclusive access without needing set_configuration().  Calling
        // set_configuration() triggers a kIOReturnAborted device reset that
        // cancels subsequent EP0 vendor requests.
        //
        // With bDeviceClass=0x00 in the firmware, macOS already creates
        // IOUSBInterface service nodes on enumeration, so claim_interface()
        // finds the interface without any prior set_configuration() call.
        let iface = device
            .claim_interface(0)
            .wait()
            .map_err(|e| AdapterError::Io(format!("claim interface 0: {e}")))?;

        // GET_INFO — verify protocol version (blocking EP0 vendor request).
        //
        // On macOS, IOKit may return kIOReturnAborted (Cancelled) on the first
        // vendor EP0 request immediately after InterfaceOpen().  Retry with
        // exponential backoff to handle this transient race.
        let info_data = {
            let mut last_err = None;
            let mut delay_ms = 50u64;
            let mut result = None;
            for attempt in 0..5 {
                match iface
                    .control_in(ctrl_in(RequestCode::GetInfo as u8, 12), CTRL_TIMEOUT)
                    .wait()
                {
                    Ok(data) => {
                        result = Some(data);
                        break;
                    }
                    Err(e) => {
                        eprintln!("KCAN: GET_INFO attempt {attempt} failed: {e:?} (retrying in {delay_ms}ms)");
                        last_err = Some(e);
                        std::thread::sleep(Duration::from_millis(delay_ms));
                        delay_ms = (delay_ms * 2).min(500);
                    }
                }
            }
            result.ok_or_else(|| {
                AdapterError::Protocol(format!("GET_INFO: {:?}", last_err.unwrap()))
            })?
        };
        if info_data.len() < 12 {
            return Err(AdapterError::Protocol("GET_INFO: short response".into()));
        }
        let info_buf: [u8; 12] = info_data[..12].try_into().unwrap();
        let info = KCanDeviceInfo::from_bytes(&info_buf);
        if info.protocol_version != 1 {
            return Err(AdapterError::Protocol(format!(
                "KCAN protocol v{} unsupported (host supports v1)",
                info.protocol_version
            )));
        }
        let (fw_maj, fw_min, fw_pat, uid) =
            (info.fw_major, info.fw_minor, info.fw_patch, info.uid_lo);
        let name = format!("KCAN Dongle v{fw_maj}.{fw_min}.{fw_pat} (uid={uid:08X})");

        // GET_BT_CONST.
        let bt_data = iface
            .control_in(ctrl_in(RequestCode::GetBtConst as u8, 32), CTRL_TIMEOUT)
            .wait()
            .map_err(|e| AdapterError::Protocol(format!("GET_BT_CONST: {e:?}")))?;
        if bt_data.len() < 32 {
            return Err(AdapterError::Protocol(
                "GET_BT_CONST: short response".into(),
            ));
        }
        let bt_buf: [u8; 32] = bt_data[..32].try_into().unwrap();
        let bt_const = KCanBtConst::from_bytes(&bt_buf);

        // SET_BITTIMING.
        let clock_hz = bt_const.clock_hz;
        let bt = KCanBitTiming::for_bitrate(clock_hz, baud).ok_or_else(|| {
            AdapterError::Protocol(format!("cannot achieve {baud} bps at {clock_hz} Hz"))
        })?;
        iface
            .control_out(
                ctrl_out(RequestCode::SetBitTiming as u8, &bt.to_bytes()),
                CTRL_TIMEOUT,
            )
            .wait()
            .map_err(|e| AdapterError::Protocol(format!("SET_BITTIMING: {e:?}")))?;

        // SET_MODE — bus on.
        let mode = KCanMode::bus_on(listen_only, false);
        iface
            .control_out(
                ctrl_out(RequestCode::SetMode as u8, &mode.to_bytes()),
                CTRL_TIMEOUT,
            )
            .wait()
            .map_err(|e| AdapterError::Protocol(format!("SET_MODE: {e:?}")))?;

        // Open bulk endpoints synchronously
        let mut ep_out = iface
            .endpoint::<Bulk, Out>(BULK_OUT_EP)
            .map_err(|e| AdapterError::Io(format!("open bulk-out ep 0x{BULK_OUT_EP:02X}: {e}")))?;
        let mut ep_in = iface
            .endpoint::<Bulk, In>(BULK_IN_EP)
            .map_err(|e| AdapterError::Io(format!("open bulk-in ep 0x{BULK_IN_EP:02X}: {e}")))?;

        // Reset data toggles on both bulk endpoints.
        //
        // On macOS, IOUSBInterfaceOpen does NOT reset host-side data toggles.
        // If the previous session ended mid-transfer, the device may be at DATA1
        // while the host starts at DATA0, causing every bulk IN packet to be
        // silently discarded (NACK loop). CLEAR_FEATURE(ENDPOINT_HALT) resets
        // both host and device toggles to DATA0 via ClearPipeStallBothEnds.
        if let Err(e) = ep_out.clear_halt().wait() {
            eprintln!("KCAN: clear_halt bulk-out: {e} (ignoring)");
        }
        if let Err(e) = ep_in.clear_halt().wait() {
            eprintln!("KCAN: clear_halt bulk-in: {e} (ignoring)");
        }

        let (frame_tx, frame_rx) = mpsc::channel::<KCanFrame>();
        let (tx_cmd_tx, tx_cmd_rx) = mpsc::sync_channel::<TxCmd>(8);
        let (error_tx, error_rx) = mpsc::sync_channel::<String>(1);

        let reader_thread = std::thread::Builder::new()
            .name("kcan-reader".into())
            .spawn(move || reader_thread(ep_in, ep_out, frame_tx, tx_cmd_rx, error_tx))
            .map_err(|e| AdapterError::Io(format!("spawn reader: {e}")))?;

        Ok(Self {
            iface,
            frame_rx,
            error_rx,
            tx_cmd_tx,
            reader_thread: Some(reader_thread),
            fw_version: (fw_maj, fw_min, fw_pat),
            name,
            tx_seq: 0,
        })
    }

    pub fn probe(serial: Option<&str>) -> bool {
        find_device_info(serial).is_ok()
    }

    pub fn list_devices() -> Vec<(String, String)> {
        let Ok(iter) = nusb::list_devices().wait() else {
            return vec![];
        };
        iter.filter(|d: &DeviceInfo| d.vendor_id() == KCAN_VID && d.product_id() == KCAN_PID)
            .map(|d: DeviceInfo| {
                let serial = d.serial_number().unwrap_or("").to_string();
                let name = d.product_string().unwrap_or("KCAN Dongle").to_string();
                (serial, name)
            })
            .collect()
    }

    fn next_seq(&mut self) -> u16 {
        let s = self.tx_seq;
        self.tx_seq = self.tx_seq.wrapping_add(1);
        s
    }

    /// Send a DFU_DETACH request to the DFU Runtime interface.
    ///
    /// The firmware's `AppDfuHandler::enter_dfu` signals the `dfu_app_task`
    /// which calls `mark_dfu()` and `sys_reset()`.  The device will reboot
    /// into the bootloader's USB DFU download mode.
    ///
    /// Returns `Ok(())` immediately after the request is sent — the device
    /// may disconnect before the response arrives, which is expected.
    pub fn enter_dfu_mode(serial: Option<&str>) -> Result<(), AdapterError> {
        let dev_info = find_device_info(serial)?;
        let device = dev_info
            .open()
            .wait()
            .map_err(|e| AdapterError::Io(format!("open device: {e}")))?;

        // Find the DFU Runtime interface (class=0xFE, subclass=0x01, protocol=0x01).
        // The KCAN app exposes it alongside the vendor CAN interface.
        // Embassy-usb adds it as interface 1 (interface 0 is the vendor CAN).
        let iface = device
            .claim_interface(1)
            .wait()
            .map_err(|e| AdapterError::Io(format!("claim DFU Runtime interface: {e}")))?;

        // DFU_DETACH (bmRequestType=0x21, bRequest=0, wValue=timeout_ms, wIndex=1)
        let _ = iface
            .control_out(
                ControlOut {
                    control_type: ControlType::Class,
                    recipient: Recipient::Interface,
                    request: 0x00, // DFU_DETACH
                    value: 1000,   // wDetachTimeOut in ms
                    index: 1,      // interface number
                    data: &[],
                },
                Duration::from_millis(500),
            )
            .wait(); // device may reset before ACKing — ignore result

        Ok(())
    }
}

impl Drop for KCanAdapter {
    fn drop(&mut self) {
        // 1. Tell the firmware we are closing (green → amber).
        let mode = kcan_protocol::control::KCanMode::bus_off();
        let _ = self
            .iface
            .control_out(
                ctrl_out(RequestCode::SetMode as u8, &mode.to_bytes()),
                CTRL_TIMEOUT,
            )
            .wait();

        // 2. Signal the reader thread to exit immediately.
        let _ = self.tx_cmd_tx.try_send(TxCmd::Shutdown);

        // 3. Wait for the reader thread to exit so it drops ep_in/ep_out and
        //    releases the OS exclusive interface claim before we return.  Without
        //    this, a rapid reconnect races claim_interface() and gets
        //    kIOReturnExclusiveAccess (0xe00002c5).
        if let Some(handle) = self.reader_thread.take() {
            let _ = handle.join();
        }
    }
}

impl CanAdapter for KCanAdapter {
    fn recv(&mut self, timeout: Duration) -> Result<ReceivedFrame, AdapterError> {
        loop {
            match self.frame_rx.recv_timeout(timeout) {
                Ok(kf) => {
                    let is_tx_echo = kf.frame_type == FrameType::TxEcho as u8;
                    // Skip Status and BusError frames; surface Data and TxEcho.
                    if kf.frame_type != FrameType::Data as u8 && !is_tx_echo {
                        continue;
                    }
                    let frame = kcan_to_can_frame(&kf).ok_or_else(|| {
                        AdapterError::Protocol("invalid CAN ID in received frame".into())
                    })?;
                    return Ok(ReceivedFrame {
                        frame,
                        hardware_timestamp_ns: Some(kf.timestamp_100ns as u64 * 100),
                        channel: kf.channel,
                        is_tx_echo,
                    });
                }
                Err(mpsc::RecvTimeoutError::Timeout) => return Err(AdapterError::Timeout),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // Reader thread exited — drain the error channel for logging.
                    // On macOS, USB disconnect can surface as TransferError::Fault
                    // or TransferError::Cancelled rather than TransferError::Disconnected,
                    // so the reader may exit without sending "__disconnected__".
                    // Treat ANY reader-thread exit as a physical disconnect; the
                    // reconnect loop will detect whether the device reappeared.
                    let reason = self.error_rx.try_recv().unwrap_or_default();
                    if !reason.is_empty() && reason != "__disconnected__" {
                        eprintln!("KCAN reader thread died: {reason} (treating as disconnect)");
                    }
                    return Err(AdapterError::Disconnected);
                }
            }
        }
    }

    fn send(&mut self, frame: &CanFrame) -> Result<(), AdapterError> {
        let seq = self.next_seq();
        let kf = can_frame_to_kcan(frame, seq);
        self.tx_cmd_tx
            .try_send(TxCmd::Send(kf.to_bytes().to_vec()))
            .map_err(|_| AdapterError::Io("KCAN TX queue full".into()))
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn firmware_version(&self) -> Option<(u8, u8, u8)> {
        Some(self.fw_version)
    }
}

// ─── Background IO thread ─────────────────────────────────────────────────────

fn reader_thread(
    mut ep_in: Endpoint<Bulk, In>,
    mut ep_out: Endpoint<Bulk, Out>,
    frame_tx: mpsc::Sender<KCanFrame>,
    tx_cmd_rx: mpsc::Receiver<TxCmd>,
    _error_tx: mpsc::SyncSender<String>,
) {
    // Accumulation buffer: holds bytes received across multiple USB packets.
    //
    // KCAN_FRAME_SIZE=80 is not a multiple of MPS.  On the h743 (USB HS via
    // ULPI, MPS=512) the firmware sends one short packet (80 bytes < 512); the
    // transfer completes in a single read.  On the h753 (USB FS, MPS=64) the
    // host splits the 80-byte frame across two packets (64 + 16).  The
    // accumulation loop below handles both cases transparently.
    let mut frame_buf: Vec<u8> = Vec::with_capacity(KCAN_FRAME_SIZE * 2);

    // Buffer size must be a multiple of MPS to pass nusb 0.2.x validation.
    // 512 satisfies both HS (MPS=512) and FS (MPS=64): 512%512=0, 512%64=0.
    const BULK_IN_BUF: usize = 512;

    loop {
        // Drain pending TX commands (non-blocking), do bulk-out for each.
        loop {
            match tx_cmd_rx.try_recv() {
                Ok(TxCmd::Send(bytes)) => {
                    let _ = ep_out.transfer_blocking(bytes.into(), Duration::from_millis(100));
                }
                Ok(TxCmd::Shutdown) => return,
                Err(_) => break,
            }
        }

        // Block on next USB packet.  Request BULK_IN_BUF bytes — a multiple of
        // the endpoint MPS (512 for HS Bulk on macOS).  nusb 0.2.x rejects
        // requests that are not a multiple of MPS with TransferError::InvalidArgument.
        // The firmware sends 64-byte + 16-byte packets; each is a short packet
        // at MPS=512, so each transfer_blocking() completes after one packet.
        match ep_in
            .transfer_blocking(Buffer::new(BULK_IN_BUF), Duration::from_millis(200))
            .into_result()
        {
            Ok(data) if !data.is_empty() => {
                frame_buf.extend_from_slice(&data);

                // Consume all complete frames sitting in the accumulation buffer.
                while frame_buf.len() >= KCAN_FRAME_SIZE {
                    if let Ok(arr) =
                        <[u8; KCAN_FRAME_SIZE]>::try_from(&frame_buf[..KCAN_FRAME_SIZE])
                    {
                        if let Some(kf) = KCanFrame::from_bytes(&arr) {
                            if frame_tx.send(kf).is_err() {
                                return; // KCanAdapter dropped — exit.
                            }
                        }
                    }
                    frame_buf.drain(..KCAN_FRAME_SIZE);
                }
            }
            Ok(_) => {} // zero-length packet — ignore
            Err(e) => {
                use nusb::transfer::TransferError;
                match e {
                    TransferError::Disconnected => {
                        // Device physically removed — signal Disconnected so the
                        // session layer can attempt a reconnect rather than fatal exit.
                        let _ = _error_tx.try_send("__disconnected__".into());
                        return;
                    }
                    TransferError::Stall => {
                        // Endpoint halted — clear it and retry once.
                        if ep_in.clear_halt().wait().is_err() {
                            let _ = _error_tx.try_send("bulk-in stall, clear_halt failed".into());
                            return;
                        }
                        frame_buf.clear(); // discard any partial frame after a stall
                    }
                    TransferError::Cancelled | TransferError::InvalidArgument => {
                        // Cancelled  = kIOReturnAborted: transient (timeout, bus reset).
                        // InvalidArgument = nusb MPS validation error or kIOReturnBadArgument.
                        //   This should not occur with BULK_IN_BUF=512 (a multiple of both
                        //   FS MPS=64 and HS MPS=512).  Retry; a true device loss surfaces
                        //   as Disconnected.
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    _ => {
                        // Fault or Unknown — not recoverable.
                        let _ = _error_tx.try_send(format!("bulk-in fatal: {e:?}"));
                        return;
                    }
                }
            }
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn ctrl_in(request: u8, length: u16) -> ControlIn {
    ControlIn {
        control_type: ControlType::Vendor,
        recipient: Recipient::Device,
        request,
        value: 0,
        index: 0,
        length,
    }
}

fn ctrl_out(request: u8, data: &[u8]) -> ControlOut<'_> {
    ControlOut {
        control_type: ControlType::Vendor,
        recipient: Recipient::Device,
        request,
        value: 0,
        index: 0,
        data,
    }
}

fn find_device_info(serial: Option<&str>) -> Result<DeviceInfo, AdapterError> {
    let iter = nusb::list_devices()
        .wait()
        .map_err(|e| AdapterError::Io(format!("USB enumeration: {e}")))?;
    for info in iter {
        if info.vendor_id() != KCAN_VID || info.product_id() != KCAN_PID {
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
        Some(s) => format!("KCAN dongle serial '{s}' not found"),
        None => "no KCAN dongle found (VID=0x1209 PID=0xBEEF)".into(),
    }))
}

fn kcan_to_can_frame(kf: &KCanFrame) -> Option<CanFrame> {
    use embedded_can::{ExtendedId, Frame, Id, StandardId};
    let dlc = kf.dlc as usize;
    let data = &kf.data[..dlc.min(8)];
    let is_eff = kf.flags & FrameFlags::EFF != 0;
    let is_rtr = kf.flags & FrameFlags::RTR != 0;
    let id: Id = if is_eff {
        Id::Extended(ExtendedId::new(kf.can_id & 0x1FFF_FFFF)?)
    } else {
        Id::Standard(StandardId::new((kf.can_id & 0x7FF) as u16)?)
    };
    if is_rtr {
        CanFrame::new_remote(id, dlc)
    } else {
        CanFrame::new(id, data)
    }
}

fn can_frame_to_kcan(frame: &CanFrame, seq: u16) -> KCanFrame {
    use embedded_can::Frame;
    use embedded_can::Id;
    let mut flags: u8 = 0;
    let can_id: u32;
    match frame.id() {
        Id::Standard(id) => {
            can_id = id.as_raw() as u32;
        }
        Id::Extended(id) => {
            can_id = id.as_raw();
            flags |= FrameFlags::EFF;
        }
    }
    if frame.is_remote_frame() {
        flags |= FrameFlags::RTR;
    }
    let data = frame.data();
    KCanFrame::new_tx(can_id, flags, data.len() as u8, data, seq)
}
