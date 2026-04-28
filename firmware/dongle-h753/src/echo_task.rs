//! Continuous bidirectional CAN echo task (feature = "periodic-echo").
//!
//! Transmits a frame on FDCAN1 and FDCAN2 alternately, 100 ms apart.
//! The frame received by the opposite channel is visible on the shared bus —
//! a PEAK PCAN-USB sniffer (or any CAN analyser) will see both IDs.
//!
//! # Frame IDs
//!
//! | Direction      | ID     | Source  | Expected RX   |
//! |----------------|--------|---------|---------------|
//! | FDCAN1 → bus   | 0x7E1  | ch 0    | FDCAN2 rx ISR |
//! | FDCAN2 → bus   | 0x7E2  | ch 1    | FDCAN1 rx ISR |
//!
//! # Payload
//!
//! 8 bytes: a little-endian u64 counter that increments with every frame,
//! allowing drop / reorder detection on the sniffer side.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use kcan_protocol::frame::KCanFrame;

use defmt::*;

const ID_FDCAN1: u32 = 0x7E1;
const ID_FDCAN2: u32 = 0x7E2;
const INTERVAL: Duration = Duration::from_millis(100);

#[embassy_executor::task]
pub async fn echo_task(
    usb_to_can: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>,
    usb_to_can2: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>,
) {
    info!("periodic-echo: started — 0x7E1 on FDCAN1, 0x7E2 on FDCAN2 @ 100 ms");

    let mut counter: u64 = 0;

    loop {
        // ── FDCAN1 TX (channel 0) ─────────────────────────────────────────────
        let payload = counter.to_le_bytes();
        let mut frame = KCanFrame::new_tx(ID_FDCAN1, 0, 8, &payload, (counter & 0xFFFF) as u16);
        frame.channel = 0;
        if usb_to_can.try_send(frame).is_err() {
            warn!(
                "periodic-echo: USB_TO_CAN full, FDCAN1 frame dropped (counter={})",
                counter
            );
        } else {
            info!("echo TX FDCAN1 [ID=0x7E1, counter={}]", counter);
        }
        counter = counter.wrapping_add(1);

        Timer::after(INTERVAL).await;

        // ── FDCAN2 TX (channel 1) ─────────────────────────────────────────────
        let payload = counter.to_le_bytes();
        let mut frame = KCanFrame::new_tx(ID_FDCAN2, 0, 8, &payload, (counter & 0xFFFF) as u16);
        frame.channel = 1;
        if usb_to_can2.try_send(frame).is_err() {
            warn!(
                "periodic-echo: USB_TO_CAN2 full, FDCAN2 frame dropped (counter={})",
                counter
            );
        } else {
            info!("echo TX FDCAN2 [ID=0x7E2, counter={}]", counter);
        }
        counter = counter.wrapping_add(1);

        Timer::after(INTERVAL).await;
    }
}
