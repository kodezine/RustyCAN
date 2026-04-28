//! Status LED task.
//!
//! | LED | Pin  | Colour | Meaning                                     |
//! |-----|------|--------|---------------------------------------------|
//! | LD1 | PB0  | Green  | Heartbeat — 1 Hz blink = firmware alive     |
//! | LD2 | PE1  | Blue   | USB host connected (solid on = enumerated)  |
//! | LD3 | PB14 | Red    | Blinks 50 ms on TX error (future)           |

use embassy_futures::join::join;
use embassy_stm32::gpio::Output;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};

use kcan_protocol::frame::KCanFrame;

#[embassy_executor::task]
pub async fn status_task(
    mut led_bus_on: Output<'static>,
    mut led_usb: Output<'static>,
    _led_err: Output<'static>,
    _can_to_usb: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>,
) {
    join(
        // LD1 — 1 Hz heartbeat blink to confirm firmware is running.
        async {
            loop {
                led_bus_on.set_high();
                Timer::after(Duration::from_millis(500)).await;
                led_bus_on.set_low();
                Timer::after(Duration::from_millis(500)).await;
            }
        },
        // LD2 — mirrors USB_CONFIGURED: solid on when a host has enumerated
        //        the device, off on disconnect/reset.
        async {
            loop {
                let configured = crate::usb_task::USB_CONFIGURED_LED.wait().await;
                if configured {
                    led_usb.set_high();
                } else {
                    led_usb.set_low();
                }
            }
        },
    )
    .await;
}
