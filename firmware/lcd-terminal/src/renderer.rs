//! DMA2D-accelerated framebuffer renderer for a 640×480 RGB565 display.
//!
//! This module drives the STM32H7 Chrom-ART DMA2D via the PAC (stm32-metapac),
//! since embassy-stm32 0.6.0 does not include a DMA2D driver.
//!
//! Two operations are hardware-accelerated:
//! - [`Renderer::fill_rect`] — register-to-memory (R2M) solid colour fill.
//! - [`Renderer::scroll_up`] — memory-to-memory (M2M) bulk copy + R2M clear.
//!
//! Individual glyph rendering is done in software (128 half-word writes per
//! 8×16 character cell) — at 400 MHz CPU it is fast enough for a scrolling
//! boot log.
//!
//! # DMA2D coherency
//!
//! The STM32H743I-EVAL runs with D-Cache **disabled** (USB DMA requirement).
//! Therefore no cache-flush/invalidate is needed before DMA2D operations.

use embassy_stm32::pac;

use crate::font::glyph;

/// Screen geometry constants.
pub const WIDTH: u16 = 640;
pub const HEIGHT: u16 = 480;

/// DMA2D mode bits for CR register.
mod mode {
    pub const R2M: u32 = 0b011; // Register-to-memory: fill
    pub const M2M_PFC: u32 = 0b001; // Memory-to-memory with PFC (copy)
}

/// RGB565 colour palette for the boot terminal.
pub mod colors {
    pub const BG_BLACK: u16 = 0x0000;
    pub const FG_WHITE: u16 = 0xFFFF;
    pub const DIM_GREEN: u16 = 0x03E0; // timestamp text
    pub const BRIGHT_GREEN: u16 = 0x07E0; // [  OK  ]
    pub const BRIGHT_RED: u16 = 0xF800; // [FAILED]
    pub const BRIGHT_YELLOW: u16 = 0xFFE0; // [ WARN ]
    pub const CYAN: u16 = 0x07FF; // banner / info
    pub const FG_DEFAULT: u16 = FG_WHITE;
}

/// Blocking DMA2D renderer.
///
/// All operations poll for completion before returning.
/// Suitable for use in an async task: the operations are short enough that
/// they do not hold the executor for more than one display line period.
pub struct Renderer {
    /// Base address of the RGB565 framebuffer in SDRAM.
    fb: *mut u16,
}

// SAFETY: Renderer is only used from one task.
unsafe impl Send for Renderer {}

impl Renderer {
    /// Create a renderer wrapping a framebuffer in SDRAM.
    ///
    /// # Safety
    /// `fb` must point to a writable region of at least WIDTH × HEIGHT × 2
    /// bytes (614 400 bytes) and must remain valid for the lifetime of `Self`.
    pub unsafe fn new(fb: *mut u16) -> Self {
        // Enable DMA2D AHB clock via RCC.
        pac::RCC.ahb3enr().modify(|w| w.set_dma2den(true));
        Self { fb }
    }

    /// Pointer to pixel (x, y) in the framebuffer.
    #[inline(always)]
    fn pixel_ptr(&self, x: u16, y: u16) -> *mut u16 {
        unsafe { self.fb.add((y as usize) * (WIDTH as usize) + (x as usize)) }
    }

    // ── DMA2D helpers ─────────────────────────────────────────────────────────

    /// Poll until DMA2D transfer complete or transfer error.
    fn dma2d_wait() {
        use embassy_stm32::pac::dma2d::vals::*;
        let d = pac::DMA2D;
        loop {
            let isr = d.isr().read();
            if isr.tcif() {
                // Clear all flags (each field takes a typed enum, not bool)
                d.ifcr().write(|w| {
                    w.set_ctcif(Ctcif::CLEAR);
                    w.set_cteif(Cteif::CLEAR);
                    w.set_caecif(Caecif::CLEAR);
                    w.set_ctwif(Ctwif::CLEAR);
                    w.set_cceif(Cceif::CLEAR);
                });
                break;
            }
            if isr.teif() {
                panic!("DMA2D transfer error");
            }
        }
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Fill a rectangle with a solid RGB565 colour.
    ///
    /// Coordinates are in pixels; (0, 0) is the top-left corner.
    pub fn fill_rect(&self, x: u16, y: u16, width: u16, height: u16, color: u16) {
        if width == 0 || height == 0 {
            return;
        }
        use embassy_stm32::pac::dma2d::vals::*;
        let d = pac::DMA2D;

        // Output pixel format: RGB565 = 2
        d.opfccr().write(|w| w.set_cm(OpfccrCm::from_bits(2)));
        // Fill colour (for R2M, OCOLR holds the colour value)
        d.ocolr().write(|w| w.set_color(color as u32));
        // Destination address
        d.omar().write(|w| w.set_ma(self.pixel_ptr(x, y) as u32));
        // Output line offset: pitch (640) minus active width
        d.oor().write(|w| w.set_lo(WIDTH - width));
        // Lines and pixels per line
        d.nlr().write(|w| {
            w.set_nl(height);
            w.set_pl(width);
        });
        // Clear interrupt flags
        d.ifcr().write(|w| {
            w.set_ctcif(Ctcif::CLEAR);
            w.set_cteif(Cteif::CLEAR);
            w.set_caecif(Caecif::CLEAR);
            w.set_ctwif(Ctwif::CLEAR);
            w.set_cceif(Cceif::CLEAR);
        });
        // Start: mode = R2M (0b011)
        d.cr().write(|w| {
            w.set_mode(Mode::from_bits(mode::R2M as u8));
            w.set_start(CrStart::START);
        });
        Self::dma2d_wait();
    }

    /// Copy a rectangular region within the framebuffer (used for scroll).
    ///
    /// Pixels from `(src_x, src_y)` are copied to `(dst_x, dst_y)`.
    /// Source and destination must not overlap in the vertical direction
    /// if copying upward (which is the scroll case).
    fn copy_rect(&self, src_x: u16, src_y: u16, dst_x: u16, dst_y: u16, width: u16, height: u16) {
        if width == 0 || height == 0 {
            return;
        }
        use embassy_stm32::pac::dma2d::vals::*;
        let d = pac::DMA2D;

        // FG (source)
        d.fgmar()
            .write(|w| w.set_ma(self.pixel_ptr(src_x, src_y) as u32));
        d.fgor().write(|w| w.set_lo(WIDTH - width));
        d.fgpfccr().write(|w| w.set_cm(FgpfccrCm::from_bits(2)));
        // Output (destination)
        d.omar()
            .write(|w| w.set_ma(self.pixel_ptr(dst_x, dst_y) as u32));
        d.oor().write(|w| w.set_lo(WIDTH - width));
        d.opfccr().write(|w| w.set_cm(OpfccrCm::from_bits(2)));
        d.nlr().write(|w| {
            w.set_nl(height);
            w.set_pl(width);
        });
        d.ifcr().write(|w| {
            w.set_ctcif(Ctcif::CLEAR);
            w.set_cteif(Cteif::CLEAR);
            w.set_caecif(Caecif::CLEAR);
            w.set_ctwif(Ctwif::CLEAR);
            w.set_cceif(Cceif::CLEAR);
        });
        // Mode = M2M_PFC (0b001), START
        d.cr().write(|w| {
            w.set_mode(Mode::from_bits(mode::M2M_PFC as u8));
            w.set_start(CrStart::START);
        });
        Self::dma2d_wait();
    }

    /// Scroll the entire display up by `rows` text rows (each row = 16 pixels).
    ///
    /// The vacated rows at the bottom are filled with `bg`.
    pub fn scroll_up(&self, rows: u16, bg: u16) {
        self.scroll_up_region(0, rows, bg);
    }

    /// Scroll only the region starting at pixel row `top_y` upward by `rows`
    /// text rows (each text row = 16 pixels).  Pixels above `top_y` are not
    /// touched.  The vacated rows at the bottom are filled with `bg`.
    pub fn scroll_up_region(&self, top_y: u16, rows: u16, bg: u16) {
        let pixels = rows * 16;
        let region_h = HEIGHT.saturating_sub(top_y);
        if pixels >= region_h {
            self.fill_rect(0, top_y, WIDTH, region_h, bg);
            return;
        }
        // Copy [top_y + pixels .. HEIGHT] → [top_y .. HEIGHT - pixels]
        self.copy_rect(0, top_y + pixels, 0, top_y, WIDTH, region_h - pixels);
        // Clear the vacated rows at the bottom
        self.fill_rect(0, HEIGHT - pixels, WIDTH, pixels, bg);
    }

    /// Blit a packed RGB565 image to the framebuffer at pixel position (x, y).
    ///
    /// `data` must contain exactly `width * height` RGB565 values, row-major,
    /// with no stride padding.  Uses DMA2D M2M with PFC (both sides RGB565).
    pub fn blit_rgb565(&self, x: u16, y: u16, width: u16, height: u16, data: &[u16]) {
        if width == 0 || height == 0 || data.len() < (width as usize * height as usize) {
            return;
        }
        use embassy_stm32::pac::dma2d::vals::*;
        let d = pac::DMA2D;

        // FG source: packed RGB565, no line-gap (tightly packed rows).
        d.fgmar().write(|w| w.set_ma(data.as_ptr() as u32));
        d.fgor().write(|w| w.set_lo(0));
        d.fgpfccr().write(|w| w.set_cm(FgpfccrCm::from_bits(2))); // RGB565

        // Output destination in framebuffer.
        d.omar().write(|w| w.set_ma(self.pixel_ptr(x, y) as u32));
        d.oor().write(|w| w.set_lo(WIDTH - width)); // framebuffer stride gap
        d.opfccr().write(|w| w.set_cm(OpfccrCm::from_bits(2))); // RGB565

        d.nlr().write(|w| {
            w.set_nl(height);
            w.set_pl(width);
        });
        d.ifcr().write(|w| {
            w.set_ctcif(Ctcif::CLEAR);
            w.set_cteif(Cteif::CLEAR);
            w.set_caecif(Caecif::CLEAR);
            w.set_ctwif(Ctwif::CLEAR);
            w.set_cceif(Cceif::CLEAR);
        });
        // M2M_PFC (0b001): memory-to-memory with pixel format conversion.
        d.cr().write(|w| {
            w.set_mode(Mode::from_bits(mode::M2M_PFC as u8));
            w.set_start(CrStart::START);
        });
        Self::dma2d_wait();
    }

    /// Draw an 8×16 glyph at character cell (col, row) in the given colours.
    ///
    /// `col` is in [0, 79], `row` is in [0, 29].  Uses software pixel writes
    /// (128 half-word stores per call — about 1 µs at 133 MHz AXI).
    pub fn draw_glyph(&self, col: u8, row: u8, ch: u8, fg: u16, bg: u16) {
        let glyph = glyph(ch);
        let px = col as u16 * 8;
        let py = row as u16 * 16;

        for r in 0..16usize {
            for c in 0..8usize {
                let color = if glyph[r * 8 + c] != 0 { fg } else { bg };
                let ptr = self.pixel_ptr(px + c as u16, py + r as u16);
                // SAFETY: ptr is inside the SDRAM framebuffer and valid
                unsafe { ptr.write_volatile(color) };
            }
        }
    }

    /// Draw an 8×16 glyph at absolute pixel position (x, y) with integer scaling.
    ///
    /// Each source pixel becomes a `scale × scale` block of half-word writes.
    /// `scale = 1` is identical to `draw_glyph` (in pixel coordinates).
    pub fn draw_glyph_scaled(&self, x: u16, y: u16, ch: u8, fg: u16, bg: u16, scale: u16) {
        let glyph = glyph(ch);
        for r in 0..16u16 {
            for c in 0..8u16 {
                let color = if glyph[r as usize * 8 + c as usize] != 0 {
                    fg
                } else {
                    bg
                };
                for sy in 0..scale {
                    for sx in 0..scale {
                        let ptr = self.pixel_ptr(x + c * scale + sx, y + r * scale + sy);
                        // SAFETY: coordinates stay within the framebuffer when the
                        // caller ensures x + 8*scale ≤ WIDTH and y + 16*scale ≤ HEIGHT.
                        unsafe { ptr.write_volatile(color) };
                    }
                }
            }
        }
    }

    /// Draw a filled circle centred at absolute pixel (cx, cy) with the given
    /// radius and colour.  Uses software pixel writes — suitable for small,
    /// infrequently redrawn indicators.
    pub fn draw_filled_circle(&self, cx: u16, cy: u16, radius: u16, color: u16) {
        let r2 = (radius as i32) * (radius as i32);
        let r = radius as i32;
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= r2 {
                    let px = cx as i32 + dx;
                    let py = cy as i32 + dy;
                    if px >= 0 && py >= 0 && (px as u16) < WIDTH && (py as u16) < HEIGHT {
                        let ptr = self.pixel_ptr(px as u16, py as u16);
                        // SAFETY: bounds checked above
                        unsafe { ptr.write_volatile(color) };
                    }
                }
            }
        }
    }

    /// Fill the entire screen with `color`.
    #[inline]
    pub fn clear(&self, color: u16) {
        self.fill_rect(0, 0, WIDTH, HEIGHT, color);
    }
}
