//! Status LED task.
//!
//! | LED | Pin  | Colour | Meaning                                     |
//! |-----|------|--------|---------------------------------------------|
//! | LD1 | PF10 | Green  | Heartbeat — 1 Hz blink = firmware alive     |
//! | LD3 | PA4  | Orange | USB host connected (solid on = enumerated)  |

use embassy_futures::join::join;
use embassy_stm32::gpio::Output;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};

use kcan_protocol::frame::KCanFrame;

#[embassy_executor::task]
pub async fn status_task(
    mut led_heartbeat: Output<'static>,
    mut led_usb: Output<'static>,
    _can_to_usb: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>,
) {
    join(
        // LD1 — 1 Hz heartbeat blink to confirm firmware is running.
        async {
            loop {
                led_heartbeat.set_high();
                Timer::after(Duration::from_millis(500)).await;
                led_heartbeat.set_low();
                Timer::after(Duration::from_millis(500)).await;
            }
        },
        // LD3 — mirrors USB_CONFIGURED: solid on when a host has enumerated
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
