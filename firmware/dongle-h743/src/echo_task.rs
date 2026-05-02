//! Continuous single-channel CAN echo task (feature = "periodic-echo").
//!
//! Transmits a frame on FDCAN1 every 100 ms.
//! A PEAK PCAN-USB sniffer (or any CAN analyser) will see ID 0x7E1 on the bus.
//!
//! # Frame ID
//!
//! | Direction    | ID    | Source |
//! |--------------|-------|--------|
//! | FDCAN1 → bus | 0x7E1 | ch 0   |
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
const INTERVAL: Duration = Duration::from_millis(100);

#[embassy_executor::task]
pub async fn echo_task(usb_to_can: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>) {
    info!("periodic-echo: started — 0x7E1 on FDCAN1 @ 100 ms");

    let mut counter: u64 = 0;

    loop {
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
    }
}
