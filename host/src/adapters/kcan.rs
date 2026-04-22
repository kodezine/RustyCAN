//! KCAN dongle adapter — pure-Rust USB via `nusb`.
//!
//! A background reader thread owns `nusb::Interface` and handles both
//! bulk IN (RX frames) and bulk OUT (TX frames).  The session thread
//! communicates via two `std::sync::mpsc` channels.

use std::sync::mpsc;
use std::time::Duration;

use nusb::transfer::{Buffer, Bulk, ControlIn, ControlOut, ControlType, In, Out, Recipient};
use nusb::{DeviceInfo, Interface, MaybeFuture};

use host_can::frame::CanFrame;

use kcan_protocol::control::{KCanBitTiming, KCanBtConst, KCanDeviceInfo, KCanMode, RequestCode};
use kcan_protocol::frame::{FrameFlags, FrameType, KCanFrame, KCAN_FRAME_SIZE};

use super::{AdapterError, CanAdapter, ReceivedFrame};

const KCAN_VID: u16 = 0x1209;
const KCAN_PID: u16 = 0xBEEF;
const BULK_IN_EP: u8 = 0x81;
const BULK_OUT_EP: u8 = 0x02;
const CTRL_TIMEOUT: Duration = Duration::from_millis(500);

enum TxCmd {
    Send(Vec<u8>),
}

pub struct KCanAdapter {
    frame_rx: mpsc::Receiver<KCanFrame>,
    tx_cmd_tx: mpsc::SyncSender<TxCmd>,
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
        let iface = device
            .claim_interface(0)
            .wait()
            .map_err(|e| AdapterError::Io(format!("claim interface 0: {e}")))?;

        // GET_INFO — verify protocol version (blocking control transfer).
        let info_data = iface
            .control_in(ctrl_in(RequestCode::GetInfo as u8, 12), CTRL_TIMEOUT)
            .wait()
            .map_err(|e| AdapterError::Protocol(format!("GET_INFO: {e:?}")))?;
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

        // Spawn background IO thread (owns Interface).
        let (frame_tx, frame_rx) = mpsc::channel::<KCanFrame>();
        let (tx_cmd_tx, tx_cmd_rx) = mpsc::sync_channel::<TxCmd>(8);

        std::thread::Builder::new()
            .name("kcan-reader".into())
            .spawn(move || reader_thread(iface, frame_tx, tx_cmd_rx))
            .map_err(|e| AdapterError::Io(format!("spawn reader: {e}")))?;

        Ok(Self {
            frame_rx,
            tx_cmd_tx,
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
}

impl CanAdapter for KCanAdapter {
    fn recv(&mut self, timeout: Duration) -> Result<ReceivedFrame, AdapterError> {
        loop {
            match self.frame_rx.recv_timeout(timeout) {
                Ok(kf) => {
                    // Skip Status/TxEcho frames — only surface Data frames to the session.
                    if kf.frame_type != FrameType::Data as u8 {
                        continue;
                    }
                    let frame = kcan_to_can_frame(&kf).ok_or_else(|| {
                        AdapterError::Protocol("invalid CAN ID in received frame".into())
                    })?;
                    return Ok(ReceivedFrame {
                        frame,
                        hardware_timestamp_us: Some(kf.timestamp_us),
                    });
                }
                Err(mpsc::RecvTimeoutError::Timeout) => return Err(AdapterError::Timeout),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(AdapterError::Io("KCAN reader thread died".into()))
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
}

// ─── Background IO thread ─────────────────────────────────────────────────────

fn reader_thread(
    iface: Interface,
    frame_tx: mpsc::Sender<KCanFrame>,
    tx_cmd_rx: mpsc::Receiver<TxCmd>,
) {
    let mut ep_out = iface
        .endpoint::<Bulk, Out>(BULK_OUT_EP)
        .expect("open bulk-out endpoint");
    let mut ep_in = iface
        .endpoint::<Bulk, In>(BULK_IN_EP)
        .expect("open bulk-in endpoint");

    loop {
        // Drain pending TX commands (non-blocking), do bulk-out for each.
        while let Ok(TxCmd::Send(bytes)) = tx_cmd_rx.try_recv() {
            let _ = ep_out.transfer_blocking(bytes.into(), Duration::from_millis(100));
        }

        // Block on next bulk IN frame (short timeout so TX can be drained regularly).
        match ep_in
            .transfer_blocking(Buffer::new(KCAN_FRAME_SIZE), Duration::from_millis(10))
            .into_result()
        {
            Ok(data) if data.len() == KCAN_FRAME_SIZE => {
                if let Ok(arr) = <[u8; KCAN_FRAME_SIZE]>::try_from(&data[..]) {
                    if let Some(kf) = KCanFrame::from_bytes(&arr) {
                        if frame_tx.send(kf).is_err() {
                            break; // KCanAdapter dropped — exit.
                        }
                    }
                }
            }
            Ok(_) => {} // wrong size — skip
            Err(_) => {
                std::thread::sleep(Duration::from_millis(10));
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
