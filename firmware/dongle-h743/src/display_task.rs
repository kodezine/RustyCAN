//! Embassy task that owns the LCD terminal and routes boot log entries.
//!
//! The header area shows a 128×128 icon, a large title, and two real-time
//! LED-style status indicators in the top-right corner:
//!
//!   ┌─────────────────────────────────────────────────────── 640px ──┐
//!   │ [icon]  RustyCAN (3×)                          [USB●] [APP●]  │
//!   │         KCAN USB-CAN Adapter                                   │
//!   │         STM32H743XI  @  400 MHz                                │
//!   │ ────────────────────────────────────────────────────────────── │
//!   │ boot log ...                                                    │
//!   └────────────────────────────────────────────────────────────────┘
//!
//! [USB●] amber = host enumerated, dark-grey = no USB.
//! [APP●] green = RustyCAN app has opened the CAN port, dark-grey otherwise.
//!
//! Other tasks post [`BootLogEntry`] to [`LOG_CHANNEL`] and USB state to
//! [`USB_STATUS`].  The main loop interleaves both with `select`.

use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use lcd_terminal::{colors, icon, BootLogEntry, LcdTerminal};

// ── Public channels / signals ─────────────────────────────────────────────────

/// Capacity of the boot-log message queue.
pub const LOG_CHANNEL_CAP: usize = 32;

/// Channel used by other tasks to push [`BootLogEntry`] messages to the LCD.
pub static LOG_CHANNEL: Channel<CriticalSectionRawMutex, BootLogEntry, LOG_CHANNEL_CAP> =
    Channel::new();

/// USB connection state reported by [`crate::ep0_handler`].
#[derive(Clone, Copy)]
pub enum UsbDisplayStatus {
    /// No USB host connected.
    Disconnected,
    /// Host has enumerated the device (SET_CONFIGURATION received).
    HostConnected,
    /// RustyCAN app has opened the CAN port (SET_MODE received).
    AppConnected,
}

/// Signalled by [`crate::ep0_handler`] on every USB state change.
/// `display_task` consumes this and redraws the header indicators.
pub static USB_STATUS: Signal<CriticalSectionRawMutex, UsbDisplayStatus> = Signal::new();

// ── Header status indicator geometry ─────────────────────────────────────────

/// Radius of the single connection-status circle.
const DOT_R: u16 = 20;
/// Centre X: right-aligned in the header.
const DOT_CX: u16 = lcd_terminal::renderer::WIDTH - 36;
/// Centre Y: vertically centred in the 144px header area.
const DOT_CY: u16 = 72;

/// Inactive (disconnected) fill colour — very dark grey.
const COLOR_OFF: u16 = 0x18C3;
/// Amber — USB host enumerated, app not open.
const COLOR_USB: u16 = 0xFD20;
/// Bright green — RustyCAN app has opened the CAN port.
const COLOR_APP: u16 = colors::BRIGHT_GREEN;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Redraw the single status circle from a [`UsbDisplayStatus`] value.
fn apply_status(lcd: &LcdTerminal, status: UsbDisplayStatus) {
    let fill = match status {
        UsbDisplayStatus::Disconnected => COLOR_OFF,
        UsbDisplayStatus::HostConnected => COLOR_USB,
        UsbDisplayStatus::AppConnected => COLOR_APP,
    };
    // Erase old circle with a solid black rectangle (DMA2D fill — no gaps).
    let erase_r = DOT_R + 2;
    lcd.fill_rect(
        DOT_CX.saturating_sub(erase_r),
        DOT_CY.saturating_sub(erase_r),
        erase_r * 2 + 1,
        erase_r * 2 + 1,
        colors::BG_BLACK,
    );
    lcd.draw_circle(DOT_CX, DOT_CY, DOT_R, fill);
}

// ── Task ──────────────────────────────────────────────────────────────────────

/// Main LCD display task.  Takes ownership of the `LcdTerminal` returned by
/// `lcd_terminal::init_or_attach()` and runs forever.
#[embassy_executor::task]
pub async fn display_task(mut lcd: LcdTerminal) {
    // ── Icon + title ──────────────────────────────────────────────────────────
    lcd.blit_image(&icon::ICON, icon::ICON_W, icon::ICON_H, 8, 8);
    lcd.write_large("RustyCAN", 148, 48, colors::CYAN, colors::BG_BLACK, 3);

    lcd.set_cursor(7, 19);
    lcd.write_colored("KCAN USB-CAN Adapter", colors::FG_WHITE, colors::BG_BLACK);
    lcd.set_cursor(8, 19);
    lcd.write_colored(
        "STM32H743XI  @  400 MHz",
        colors::DIM_GREEN,
        colors::BG_BLACK,
    );

    // ── Initial status indicators (both off) ──────────────────────────────────
    apply_status(&lcd, UsbDisplayStatus::Disconnected);

    // ── Full-width separator + cursor start ───────────────────────────────────
    lcd.set_cursor(9, 0);
    lcd.write_colored(
        "--------------------------------------------------------------------------------",
        colors::CYAN,
        colors::BG_BLACK,
    );
    lcd.set_cursor(10, 0);
    // Lock the header: rows 0–9 are never scrolled or overwritten.
    lcd.set_log_start_row(10);

    // ── Main loop: interleave log entries and status updates ──────────────────
    loop {
        match select(LOG_CHANNEL.receive(), USB_STATUS.wait()).await {
            Either::First(entry) => lcd.boot_log(entry),
            Either::Second(status) => apply_status(&lcd, status),
        }
    }
}
