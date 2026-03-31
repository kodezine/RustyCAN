//! FDCAN1 task — receives CAN frames and forwards TX requests.

use embassy_stm32::can::frame::Frame;
use embassy_stm32::can::Can;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::Instant;
use embedded_can::{ExtendedId, Id, StandardId};

use kcan_protocol::frame::{FrameFlags, KCanFrame};

use defmt::*;

#[embassy_executor::task]
pub async fn can_task(
    can: Can<'static>,
    can_to_usb: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>,
    usb_to_can: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>,
) {
    let (mut tx, mut rx, _) = can.split();

    info!("FDCAN1 started at 250 kbps (default; host will reconfigure)");

    let mut rx_seq: u16 = 0;
    let mut echo_seq: u16 = 0;

    loop {
        use embassy_futures::select::{select, Either};

        match select(rx.read(), usb_to_can.receive()).await {
            Either::First(rx_result) => match rx_result {
                Ok(envelope) => {
                    let ts_us = Instant::now().as_micros() as u32;
                    let (frame, _ts) = envelope.parts();
                    let kf = classic_to_kcan(&frame, ts_us, rx_seq);
                    rx_seq = rx_seq.wrapping_add(1);
                    if can_to_usb.try_send(kf).is_err() {
                        warn!("can_to_usb channel full — RX frame dropped");
                    }
                }
                Err(e) => {
                    warn!("FDCAN RX error: {:?}", e);
                }
            },
            Either::Second(kf) => {
                if let Some(frame) = kcan_to_frame(&kf) {
                    // write() returns Option<Frame> (Some = replaced old pending frame, None = sent cleanly)
                    tx.write(&frame).await;
                    let ts_us = Instant::now().as_micros() as u32;
                    let echo = KCanFrame::new_tx_echo(
                        kf.can_id,
                        kf.flags,
                        kf.dlc,
                        &kf.data[..kf.dlc as usize],
                        ts_us,
                        echo_seq,
                    );
                    echo_seq = echo_seq.wrapping_add(1);
                    let _ = can_to_usb.try_send(echo);
                }
            }
        }
    }
}

// ─── Frame conversion helpers ─────────────────────────────────────────────────

fn classic_to_kcan(frame: &Frame, timestamp_us: u32, seq: u16) -> KCanFrame {
    let header = frame.header();
    let (id_val, mut flags) = match header.id() {
        Id::Standard(id) => (id.as_raw() as u32, 0u8),
        Id::Extended(id) => (id.as_raw(), FrameFlags::EFF),
    };
    if header.rtr() {
        flags |= FrameFlags::RTR;
    }
    let data = frame.data();
    let dlc = header.len();
    KCanFrame::new_data(id_val, flags, dlc, data, timestamp_us, seq)
}

fn kcan_to_frame(kf: &KCanFrame) -> Option<Frame> {
    let dlc = kf.dlc as usize;
    let is_eff = kf.flags & FrameFlags::EFF != 0;
    let is_rtr = kf.flags & FrameFlags::RTR != 0;

    if is_rtr {
        if is_eff {
            Frame::new_remote(ExtendedId::new(kf.can_id & 0x1FFF_FFFF).unwrap(), dlc).ok()
        } else {
            Frame::new_remote(StandardId::new(kf.can_id as u16 & 0x7FF).unwrap(), dlc).ok()
        }
    } else {
        let data = &kf.data[..dlc.min(8)];
        if is_eff {
            Frame::new_extended(kf.can_id, data).ok()
        } else {
            Frame::new_standard(kf.can_id as u16, data).ok()
        }
    }
}
