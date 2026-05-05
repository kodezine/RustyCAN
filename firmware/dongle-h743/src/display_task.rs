//! Embassy task that owns the LCD terminal and routes boot log entries.
//!
//! The header area shows a 128×128 icon, a large title, one connection-status
//! dot, and a live stats row that updates every second:
//!
//!   ┌─────────────────────────────────────────────────────── 640px ──┐
//!   │ [icon]  RustyCAN (3×, y=32)                             [●]   │
//!   │         KCAN USB-CAN Adapter                        (row 6)   │
//!   │         STM32H743XI  @  400 MHz                     (row 7)   │
//!   │         250 kbps | 7 src | 124 fps                  (row 8)   │
//!   │ ────────────────────────────────────────────────────────────── │  (row 9)
//!   │ boot log ...                                                    │
//!   └────────────────────────────────────────────────────────────────┘
//!
//! [●] dark-grey = no USB; amber = host enumerated; green = app opened port.
//!
//! Other tasks post [`BootLogEntry`] to [`LOG_CHANNEL`] and USB state to
//! [`USB_STATUS`].  [`crate::can_task`] increments [`RX_FRAME_COUNTER`] and
//! updates [`SEEN_IDS`] per frame.  [`crate::ep0_handler`] writes [`BAUD_KBPS`]
//! on SET_BITTIMING.  The main loop uses a 1-second ticker to redraw row 8.

use core::sync::atomic::{AtomicU32, Ordering};
use embassy_futures::select::{select3, Either3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Ticker};
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

// ── Live CAN stats (written by can_task / ep0_handler, read by display_task) ──

/// CAN baud rate in kbps — written by [`crate::ep0_handler`] on SET_BITTIMING.
/// Initialised to 250 because the firmware hardcodes 250 kbps at boot.
pub static BAUD_KBPS: AtomicU32 = AtomicU32::new(250);

/// RX frame counter — incremented by [`crate::can_task`] per received frame,
/// swapped to zero by `display_task` each second to compute fps.
pub static RX_FRAME_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Bitset of seen standard (11-bit) CAN IDs.  64 × u32 = 2048 bits, one per ID.
/// Written by [`crate::can_task`] via `fetch_or`; never cleared (cumulative).
pub static SEEN_IDS: [AtomicU32; 64] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: AtomicU32 = AtomicU32::new(0);
    [ZERO; 64]
};

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
    lcd.write_large("RustyCAN", 148, 32, colors::CYAN, colors::BG_BLACK, 3);

    lcd.set_cursor(6, 19);
    lcd.write_colored("KCAN USB-CAN Adapter", colors::FG_WHITE, colors::BG_BLACK);
    lcd.set_cursor(7, 19);
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

    // ── Main loop: interleave log entries, status updates, and 1-s stats tick ─
    let mut ticker = Ticker::every(Duration::from_secs(1));
    loop {
        match select3(LOG_CHANNEL.receive(), USB_STATUS.wait(), ticker.next()).await {
            Either3::First(entry) => lcd.boot_log(entry),
            Either3::Second(status) => apply_status(&lcd, status),
            Either3::Third(_) => update_stats_row(&mut lcd),
        }
    }
}

// ── Live stats helpers ────────────────────────────────────────────────────────

/// Read stats atomics, swap fps counter to zero, and redraw row 8 col 19.
fn update_stats_row(lcd: &mut LcdTerminal) {
    let baud = BAUD_KBPS.load(Ordering::Relaxed);
    let fps = RX_FRAME_COUNTER.swap(0, Ordering::Relaxed);
    let src: u16 = SEEN_IDS
        .iter()
        .map(|w| w.load(Ordering::Relaxed).count_ones() as u16)
        .sum();
    let mut buf = [b' '; 60];
    let text = format_stats_buf(&mut buf, baud, src, fps);
    lcd.set_cursor(8, 19);
    lcd.write_colored(text, colors::DIM_GREEN, colors::BG_BLACK);
}

/// Format `baud_kbps | src | fps` into a 60-byte space-padded ASCII buffer.
fn format_stats_buf(buf: &mut [u8; 60], baud_kbps: u32, src: u16, fps: u32) -> &str {
    for b in buf.iter_mut() {
        *b = b' ';
    }
    let mut pos = 0usize;
    fmt_u32(buf, &mut pos, baud_kbps);
    fmt_bytes(buf, &mut pos, b" kbps | ");
    fmt_u32(buf, &mut pos, src as u32);
    fmt_bytes(buf, &mut pos, b" src | ");
    fmt_u32(buf, &mut pos, fps);
    fmt_bytes(buf, &mut pos, b" fps");
    // SAFETY: all written bytes are printable ASCII (0x20–0x39), valid UTF-8.
    unsafe { core::str::from_utf8_unchecked(buf) }
}

fn fmt_bytes(buf: &mut [u8], pos: &mut usize, s: &[u8]) {
    for &b in s {
        if *pos < buf.len() {
            buf[*pos] = b;
            *pos += 1;
        }
    }
}

fn fmt_u32(buf: &mut [u8], pos: &mut usize, mut n: u32) {
    if n == 0 {
        fmt_bytes(buf, pos, b"0");
        return;
    }
    let start = *pos;
    while n > 0 && *pos < buf.len() {
        buf[*pos] = b'0' + (n % 10) as u8;
        *pos += 1;
        n /= 10;
    }
    buf[start..*pos].reverse();
}
