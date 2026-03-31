//! USB tasks.
//!
//! Two tasks:
//! 1. `usb_device_task`  — runs `UsbDevice::run()` forever (handles enumeration,
//!    suspend/resume, EP0 standard requests).
//! 2. `kcan_io_task`     — bridges Bulk IN/OUT to the CAN channels:
//!    - Bulk OUT → `usb_to_can` channel (TX frames from host)
//!    - `can_to_usb` channel → Bulk IN (RX frames + TX echoes to host)

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_usb::UsbDevice;

use kcan_protocol::frame::{KCanFrame, KCAN_FRAME_SIZE};

use defmt::*;

#[embassy_executor::task]
pub async fn usb_device_task(
    mut usb: UsbDevice<
        'static,
        embassy_stm32::usb::Driver<'static, embassy_stm32::peripherals::USB_OTG_FS>,
    >,
) {
    usb.run().await;
}

#[embassy_executor::task]
pub async fn kcan_io_task(
    class: crate::kcan_usb::KCanUsbClass<
        'static,
        embassy_stm32::usb::Driver<'static, embassy_stm32::peripherals::USB_OTG_FS>,
    >,
    can_to_usb: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>,
    usb_to_can: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>,
) {
    let (mut sender, mut receiver) = class.split();

    // Run USB IN and OUT concurrently.
    use embassy_futures::join::join;

    join(
        // USB Bulk IN: drain can_to_usb → send to host.
        async {
            loop {
                let frame = can_to_usb.receive().await;
                let bytes = frame.to_bytes();
                if let Err(e) = sender.write_packet(&bytes).await {
                    warn!("USB Bulk IN write error: {:?}", e);
                }
            }
        },
        // USB Bulk OUT: read from host → push to usb_to_can.
        async {
            let mut buf = [0u8; KCAN_FRAME_SIZE];
            loop {
                match receiver.read_packet(&mut buf).await {
                    Ok(n) if n == KCAN_FRAME_SIZE => {
                        let arr: [u8; KCAN_FRAME_SIZE] = buf;
                        if let Some(frame) = KCanFrame::from_bytes(&arr) {
                            if usb_to_can.try_send(frame).is_err() {
                                warn!("usb_to_can channel full — TX frame dropped");
                            }
                        } else {
                            warn!("USB Bulk OUT: bad magic/version in received frame");
                        }
                    }
                    Ok(n) => {
                        warn!("USB Bulk OUT: unexpected packet size {}", n);
                    }
                    Err(e) => {
                        warn!("USB Bulk OUT read error: {:?}", e);
                    }
                }
            }
        },
    )
    .await;
}
