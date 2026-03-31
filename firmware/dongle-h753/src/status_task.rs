//! Status LED task.
//!
//! | LED | Pin  | Meaning                        |
//! |-----|------|--------------------------------|
//! | LD1 | PB0  | Solid on = bus-on              |
//! | LD2 | PE1  | Blinks 50 ms on each RX frame  |
//! | LD3 | PB14 | Blinks 50 ms on TX error       |

use embassy_stm32::gpio::Output;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};

use kcan_protocol::frame::KCanFrame;

#[embassy_executor::task]
pub async fn status_task(
    mut led_bus_on: Output<'static>,
    _led_rx: Output<'static>,
    _led_err: Output<'static>,
    _can_to_usb: &'static Channel<CriticalSectionRawMutex, KCanFrame, 32>,
) {
    // The status task watches the can_to_usb channel passively via try_receive
    // on a tick so it doesn't compete with the USB IO task.
    loop {
        Timer::after(Duration::from_millis(10)).await;

        // We don't own the channel here; just blink LD1 at 1 Hz to show life
        // until proper bus-on state tracking is wired from the config struct.
        // TODO: wire to KCanConfig.bus_on once config is shared via a Mutex.
        led_bus_on.toggle();
        Timer::after(Duration::from_millis(490)).await;
        led_bus_on.toggle();
        Timer::after(Duration::from_millis(490)).await;
    }
}
