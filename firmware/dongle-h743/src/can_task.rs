//! FDCAN1 task — receives CAN frames and forwards TX requests.
//!
//! Single-channel variant for STM32H743I-EVAL (MB1246 Rev E).
//! FDCAN1 is connected to the on-board TJA1044 transceiver via CN3 DB9.

use embassy_stm32::can::frame::Frame;
use embassy_stm32::can::Can;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{with_timeout, Duration, Instant};
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

    info!("FDCAN1 started at 250 kbps");

    // ── Phase 2 loopback self-test ────────────────────────────────────────────
    // Sends a known test frame and expects it back immediately via internal
    // loopback to verify clock/timing/frame-path without external hardware.
    #[cfg(feature = "loopback")]
    {
        use embassy_time::{with_timeout, Duration};
        const TEST_ID: u16 = 0x123;
        const TEST_DATA: [u8; 8] = [0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03, 0x04];
        if let Ok(test_frame) = Frame::new_standard(TEST_ID, &TEST_DATA) {
            tx.write(&test_frame).await;
            match with_timeout(Duration::from_millis(100), rx.read()).await {
                Ok(Ok(envelope)) => {
                    let (rx_frame, _) = envelope.parts();
                    if rx_frame.data() == &TEST_DATA {
                        info!(
                            "FDCAN self-test: PASS [ID={:#05x}, loopback RX matched TX]",
                            TEST_ID
                        );
                    } else {
                        error!("FDCAN self-test: FAIL [data mismatch]");
                    }
                }
                Ok(Err(e)) => error!("FDCAN self-test: FAIL [RX error: {:?}]", e),
                Err(_) => error!("FDCAN self-test: FAIL [timeout 100 ms — check 32 MHz PLL2Q]"),
            }
        }
    }

    let mut rx_seq: u16 = 0;
    let mut echo_seq: u16 = 0;

    loop {
        use embassy_futures::select::{select, Either};

        match select(rx.read(), usb_to_can.receive()).await {
            Either::First(rx_result) => match rx_result {
                Ok(envelope) => {
                    // Use the ISR-captured timestamp (TIM2 at 1 MHz, time-driver-tim2).
                    // This is more accurate than Instant::now() read in the async task.
                    let (frame, rx_ts) = envelope.parts();
                    let ts_us = rx_ts.as_micros() as u32;
                    let header = frame.header();
                    let id_val = match header.id() {
                        Id::Standard(id) => id.as_raw() as u32,
                        Id::Extended(id) => id.as_raw(),
                    };
                    info!("FDCAN RX [ID={:#010x}, DLC={}]", id_val, header.len());
                    let mut kf = classic_to_kcan(&frame, ts_us, rx_seq);
                    kf.channel = 0;
                    rx_seq = rx_seq.wrapping_add(1);
                    // Under periodic-echo there is no USB host draining the
                    // channel; silently drop to avoid log spam.
                    #[cfg(not(feature = "periodic-echo"))]
                    if can_to_usb.try_send(kf).is_err() {
                        warn!("can_to_usb channel full — RX frame dropped");
                    }
                    #[cfg(feature = "periodic-echo")]
                    let _ = can_to_usb.try_send(kf);
                }
                Err(e) => {
                    warn!("FDCAN RX error: {:?}", e);
                }
            },
            Either::Second(kf) => {
                if let Some(frame) = kcan_to_frame(&kf) {
                    info!("FDCAN TX [ID={:#010x}, DLC={}]", kf.can_id, kf.dlc);
                    // write() blocks until TX FIFO has space; guard with a timeout
                    // so a Bus-Off state doesn't permanently stall this task.
                    if with_timeout(Duration::from_millis(200), tx.write(&frame))
                        .await
                        .is_err()
                    {
                        warn!("FDCAN1 TX timeout — possible Bus-Off, skipping frame");
                        continue;
                    }
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
