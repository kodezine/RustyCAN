//! `lcd-terminal` — shared no_std LCD boot terminal for STM32H7.
//!
//! This crate provides a scrolling dmesg-style boot console driven by the
//! STM32H7 LTDC peripheral (640×480 RGB565) and DMA2D Chrom-ART hardware
//! acceleration. It is designed to be shared between the bootloader and any
//! downstream firmware via a [`LcdHandoff`] record in SRAM4.
//!
//! # Quick start (from `dongle-h743/src/main.rs`)
//!
//! ```rust,ignore
//! // 1. Caller initialises SDRAM and gets the *mut u16 base pointer.
//! let fb = init_sdram_for_eval(...) as *mut u16;
//!
//! // 2. Pass fb to init_or_attach; it owns LTDC from here.
//! let lcd = lcd_terminal::init_or_attach(fb, p.LTDC, irqs, ...);
//! spawner.spawn(display_task(lcd)).unwrap();
//!
//! // From any task:
//! boot_log!(LOG_CHANNEL, "FDCAN1 ready", BootStatus::Ok);
//! ```
//!
//! # SDRAM responsibility
//!
//! The caller is responsible for initialising SDRAM before calling
//! `init_or_attach`.  `sdram::init_sdram` (re-exported as
//! `lcd_terminal::sdram::init_sdram`) is provided as a helper, but the caller
//! must pass all the FMC pins directly (Rust does not allow `impl Trait` in
//! struct fields on stable).
//!
//! # Memory layout additions
//!
//! The `build.rs` of this crate emits `lcd_handoff.x`, which inserts a
//! `.lcd_handoff` NOLOAD section into SRAM4 (`0x3800_0000`).  The SDRAM and
//! SRAM4 MEMORY regions must be declared in `firmware/memory.x` (already done).
//!
//! # Pixel clock
//!
//! PLL3 must be configured in `main.rs` **before** calling `init_or_attach`.
//! Target: HSE 25 MHz, M=5, N=160, R=32 → 25 MHz on PLL3R fed to LTDC.

#![no_std]

pub mod console;
pub mod font;
pub mod handoff;
pub mod icon;
pub mod ltdc;
pub mod renderer;
pub mod sdram;

pub use console::{BootLogEntry, BootStatus, Console, COLS, ROWS};
pub use renderer::colors;

use handoff::LcdHandoff;
use renderer::Renderer;

/// Fully-initialised LCD terminal handle.
///
/// Owns the [`Console`] cell grid and the [`Renderer`].
/// The LTDC peripheral continues to scan out the framebuffer in SDRAM
/// independently; this struct is only needed to write new text.
pub struct LcdTerminal {
    renderer: Renderer,
    console: Console,
}

impl LcdTerminal {
    /// Write a string to the terminal (no trailing newline).
    pub fn write(&mut self, s: &str) {
        self.console
            .write_str(s, colors::FG_WHITE, colors::BG_BLACK, &self.renderer);
    }

    /// Write a string followed by a newline.
    pub fn writeln(&mut self, s: &str) {
        self.console
            .writeln(s, colors::FG_WHITE, colors::BG_BLACK, &self.renderer);
    }

    /// Render a structured boot log entry on its own line.
    pub fn boot_log(&mut self, entry: BootLogEntry) {
        self.console.write_boot_log(&entry, &self.renderer);
    }

    /// Clear the screen and move the cursor to (0, 0).
    pub fn clear(&mut self) {
        self.console.clear(&self.renderer);
    }

    /// Write a coloured string.
    pub fn write_colored(&mut self, s: &str, fg: u16, bg: u16) {
        self.console.write_str(s, fg, bg, &self.renderer);
    }

    /// Write a string at absolute pixel position (x, y) using scaled glyphs.
    ///
    /// Each character is rendered at `8 * scale` × `16 * scale` pixels.
    /// Does **not** update the character-cell cursor.
    /// Only ASCII/CP437 single-byte characters are supported (passed as `u8`).
    pub fn write_large(&self, s: &str, x: u16, y: u16, fg: u16, bg: u16, scale: u16) {
        let mut cx = x;
        for b in s.bytes() {
            self.renderer.draw_glyph_scaled(cx, y, b, fg, bg, scale);
            cx += 8 * scale;
        }
    }

    /// Blit a packed RGB565 image to the framebuffer at pixel position (x, y).
    ///
    /// `data` must contain `width * height` RGB565 values (row-major, no stride).
    /// Pixel coordinates are absolute — (0, 0) is the top-left of the screen.
    pub fn blit_image(&self, data: &[u16], width: u16, height: u16, x: u16, y: u16) {
        self.renderer.blit_rgb565(x, y, width, height, data);
    }

    /// Fill a rectangle on the framebuffer with a solid RGB565 colour.
    ///
    /// Coordinates are absolute pixel positions; (0, 0) is top-left.
    pub fn fill_rect(&self, x: u16, y: u16, width: u16, height: u16, color: u16) {
        self.renderer.fill_rect(x, y, width, height, color);
    }

    /// Draw a filled circle at absolute pixel centre (cx, cy) with the given radius.
    pub fn draw_circle(&self, cx: u16, cy: u16, radius: u16, color: u16) {
        self.renderer.draw_filled_circle(cx, cy, radius, color);
    }

    /// Move the text cursor to (row, col), clamped to the console bounds.
    pub fn set_cursor(&mut self, row: u8, col: u8) {
        self.console.row = row.min(ROWS - 1);
        self.console.col = col.min(COLS - 1);
    }

    /// Set the first row reserved for scrolling log output.
    /// Rows 0..row are the header and will never be scrolled or overwritten.
    /// Also moves the cursor to that row if it is currently above it.
    pub fn set_log_start_row(&mut self, row: u8) {
        let row = row.min(ROWS - 1);
        self.console.top_row = row;
        if self.console.row < row {
            self.console.row = row;
        }
    }

    /// Current cursor row (0-based).
    pub fn row(&self) -> u8 {
        self.console.row
    }

    /// Current cursor column (0-based).
    pub fn col(&self) -> u8 {
        self.console.col
    }
}

// ── init_or_attach ────────────────────────────────────────────────────────────

/// Attach the LCD terminal to an SDRAM framebuffer and start the LTDC
/// scan-out.
///
/// # Caller responsibilities
///
/// 1. Configure PLL3R = 25 MHz in `embassy_stm32::Config::rcc.pll3` **before**
///    calling `embassy_stm32::init()`.
/// 2. Initialise FMC SDRAM (e.g. via `lcd_terminal::sdram::init_sdram`) and
///    pass the resulting `*mut u16` base pointer as `fb`.
///
/// # Cold vs warm boot
///
/// - **Cold boot** (no valid [`LcdHandoff`] magic): clears the framebuffer to
///   black, then starts LTDC.
/// - **Warm boot** (magic = `0xCAFE_FEED`): reuses the existing framebuffer,
///   restores cursor, and just re-enables LTDC from the saved state.
///
/// # Safety
///
/// `fb` must point to a writable, non-cached region of at least
/// `640 × 480 × 2 = 614 400` bytes, valid for `'static`.
#[allow(clippy::too_many_arguments)]
pub unsafe fn init_or_attach<'d>(
    // Pre-initialised RGB565 framebuffer in SDRAM.
    fb: *mut u16,
    ltdc_peri: embassy_stm32::Peri<'d, embassy_stm32::peripherals::LTDC>,
    ltdc_irq: impl embassy_stm32::interrupt::typelevel::Binding<
            embassy_stm32::interrupt::typelevel::LTDC,
            embassy_stm32::ltdc::InterruptHandler<embassy_stm32::peripherals::LTDC>,
        > + 'd,
    // Backlight
    bl_ctrl: embassy_stm32::Peri<'d, impl embassy_stm32::gpio::Pin>,
    // DE pin (PK7 AF14)
    de: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::DePin<embassy_stm32::peripherals::LTDC>>,
    // Sync / clock
    clk: embassy_stm32::Peri<
        'd,
        impl embassy_stm32::ltdc::ClkPin<embassy_stm32::peripherals::LTDC>,
    >,
    hsync: embassy_stm32::Peri<
        'd,
        impl embassy_stm32::ltdc::HsyncPin<embassy_stm32::peripherals::LTDC>,
    >,
    vsync: embassy_stm32::Peri<
        'd,
        impl embassy_stm32::ltdc::VsyncPin<embassy_stm32::peripherals::LTDC>,
    >,
    // Red R0–R7
    r0: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::R0Pin<embassy_stm32::peripherals::LTDC>>,
    r1: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::R1Pin<embassy_stm32::peripherals::LTDC>>,
    r2: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::R2Pin<embassy_stm32::peripherals::LTDC>>,
    r3: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::R3Pin<embassy_stm32::peripherals::LTDC>>,
    r4: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::R4Pin<embassy_stm32::peripherals::LTDC>>,
    r5: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::R5Pin<embassy_stm32::peripherals::LTDC>>,
    r6: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::R6Pin<embassy_stm32::peripherals::LTDC>>,
    r7: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::R7Pin<embassy_stm32::peripherals::LTDC>>,
    // Green G0–G7
    g0: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::G0Pin<embassy_stm32::peripherals::LTDC>>,
    g1: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::G1Pin<embassy_stm32::peripherals::LTDC>>,
    g2: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::G2Pin<embassy_stm32::peripherals::LTDC>>,
    g3: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::G3Pin<embassy_stm32::peripherals::LTDC>>,
    g4: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::G4Pin<embassy_stm32::peripherals::LTDC>>,
    g5: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::G5Pin<embassy_stm32::peripherals::LTDC>>,
    g6: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::G6Pin<embassy_stm32::peripherals::LTDC>>,
    g7: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::G7Pin<embassy_stm32::peripherals::LTDC>>,
    // Blue B0–B7
    b0: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::B0Pin<embassy_stm32::peripherals::LTDC>>,
    b1: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::B1Pin<embassy_stm32::peripherals::LTDC>>,
    b2: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::B2Pin<embassy_stm32::peripherals::LTDC>>,
    b3: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::B3Pin<embassy_stm32::peripherals::LTDC>>,
    b4: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::B4Pin<embassy_stm32::peripherals::LTDC>>,
    b5: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::B5Pin<embassy_stm32::peripherals::LTDC>>,
    b6: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::B6Pin<embassy_stm32::peripherals::LTDC>>,
    b7: embassy_stm32::Peri<'d, impl embassy_stm32::ltdc::B7Pin<embassy_stm32::peripherals::LTDC>>,
) -> LcdTerminal {
    let warm = unsafe { LcdHandoff::is_valid() };

    if !warm {
        unsafe { LcdHandoff::mark_cold() };
    }

    // Create renderer (enables DMA2D clock, wraps fb pointer)
    let renderer = unsafe { Renderer::new(fb) };

    // Cold boot: clear framebuffer before LTDC starts
    if !warm {
        renderer.clear(colors::BG_BLACK);
    }

    // ── LTDC ──────────────────────────────────────────────────────────────────
    let _ltdc = ltdc::init_ltdc(
        ltdc_peri, ltdc_irq, bl_ctrl, de, clk, hsync, vsync, r0, r1, r2, r3, r4, r5, r6, r7, g0,
        g1, g2, g3, g4, g5, g6, g7, b0, b1, b2, b3, b4, b5, b6, b7, fb as u32,
    );
    // Leak the LTDC handle — the peripheral runs autonomously
    core::mem::forget(_ltdc);

    // ── Console state ─────────────────────────────────────────────────────────
    let mut console = Console::new();
    if warm {
        let h = unsafe { handoff::LCD_HANDOFF.cursor_row };
        let c = unsafe { handoff::LCD_HANDOFF.cursor_col };
        console.row = h;
        console.col = c;
    }

    // ── Update handoff ────────────────────────────────────────────────────────
    unsafe {
        LcdHandoff::commit(
            fb as u32,
            console.row,
            console.col,
            colors::FG_WHITE,
            colors::BG_BLACK,
        );
    }

    LcdTerminal { renderer, console }
}

// ── boot_log! macro ──────────────────────────────────────────────────────────

/// Send a boot log entry to the display task's channel.
///
/// ```rust,ignore
/// boot_log!(LOG_CHANNEL, "FDCAN1 ready", BootStatus::Ok);
/// ```
///
/// The macro calls `embassy_time::Instant::now().as_micros()` for the
/// timestamp.
#[macro_export]
macro_rules! boot_log {
    ($channel:expr, $text:expr, $status:expr) => {
        $channel
            .try_send($crate::BootLogEntry {
                timestamp_us: embassy_time::Instant::now().as_micros(),
                text: $text,
                status: $status,
            })
            .ok()
    };
}
