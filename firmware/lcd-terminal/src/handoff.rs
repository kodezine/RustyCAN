//! LcdHandoff — shared state that persists across a CPU reset.
//!
//! Placed at the base of SRAM4 (`0x3800_0000`) via the `.lcd_handoff` NOLOAD
//! section defined in `lcd_handoff.x`.  cortex-m-rt never zeroes NOLOAD
//! sections, so the struct survives a bootloader→firmware software reset.
//!
//! # Protocol
//!
//! 1. A cold-boot firmware calls [`LcdHandoff::mark_cold`] to invalidate any
//!    stale magic before hardware init.
//! 2. After successful LTDC + SDRAM init it calls [`LcdHandoff::commit`] to
//!    write the valid magic and current display state.
//! 3. The next firmware reads [`LcdHandoff::is_valid`].  If `true` it calls
//!    [`init_or_attach`][crate::init_or_attach] which **skips** hardware
//!    re-initialisation, reuses the existing framebuffer, and continues
//!    rendering from the saved cursor position.

use core::sync::atomic::{compiler_fence, Ordering};

/// Magic value written by a firmware after successful LTDC/SDRAM init.
/// Chosen to be highly unlikely as uninitialised SRAM garbage.
pub const HANDOFF_MAGIC: u32 = 0xCAFE_FEED;

/// Persistent display state stored in SRAM4 `.lcd_handoff` NOLOAD section.
///
/// `repr(C)` guarantees a stable layout that every binary in the chain
/// (bootloader, aux firmware, main firmware) agrees on.
#[repr(C)]
pub struct LcdHandoff {
    /// Set to [`HANDOFF_MAGIC`] only after a successful display init.
    pub magic: u32,
    /// Base address of the RGB565 framebuffer in SDRAM.
    pub fb_addr: u32,
    /// Current text cursor row (0-based, 0–29).
    pub cursor_row: u8,
    /// Current text cursor column (0-based, 0–79).
    pub cursor_col: u8,
    /// Current foreground colour (RGB565).
    pub fg_color: u16,
    /// Current background colour (RGB565).
    pub bg_color: u16,
    /// Number of times a firmware has successfully attached. Diagnostic only.
    pub init_count: u32,
}

/// The single instance, placed in SRAM4 via the `lcd_handoff.x` linker snippet.
///
/// # Safety
///
/// Access must occur only after the `.lcd_handoff` section has been mapped
/// (always true after reset on STM32H7) and must be serialised by the caller
/// (single-core usage; no concurrent access).
#[unsafe(link_section = ".lcd_handoff")]
pub static mut LCD_HANDOFF: LcdHandoff = LcdHandoff {
    magic: 0,
    fb_addr: 0,
    cursor_row: 0,
    cursor_col: 0,
    fg_color: 0xFFFF, // white
    bg_color: 0x0000, // black
    init_count: 0,
};

impl LcdHandoff {
    /// Returns `true` if a previous firmware left valid display state.
    ///
    /// # Safety
    /// Caller must ensure no concurrent mutation of `LCD_HANDOFF`.
    #[inline]
    pub unsafe fn is_valid() -> bool {
        compiler_fence(Ordering::Acquire);
        // SAFETY: single-core, no IRQ touches this region.
        unsafe { core::ptr::read_volatile(&raw const LCD_HANDOFF.magic) == HANDOFF_MAGIC }
    }

    /// Invalidate the handoff record — call before any hardware init to avoid
    /// a half-initialised state from a previous crash being treated as valid.
    ///
    /// # Safety
    /// Caller must ensure no concurrent mutation of `LCD_HANDOFF`.
    #[inline]
    pub unsafe fn mark_cold() {
        // SAFETY: single-core, exclusive access guaranteed by caller.
        unsafe { core::ptr::write_volatile(&raw mut LCD_HANDOFF.magic, 0) };
        compiler_fence(Ordering::Release);
    }

    /// Write valid state after successful hardware init.
    ///
    /// # Safety
    /// Caller must ensure no concurrent mutation of `LCD_HANDOFF`.
    pub unsafe fn commit(fb_addr: u32, cursor_row: u8, cursor_col: u8, fg: u16, bg: u16) {
        // SAFETY: single-core, exclusive access.
        unsafe {
            let h = &raw mut LCD_HANDOFF;
            core::ptr::write_volatile(&raw mut (*h).fb_addr, fb_addr);
            core::ptr::write_volatile(&raw mut (*h).cursor_row, cursor_row);
            core::ptr::write_volatile(&raw mut (*h).cursor_col, cursor_col);
            core::ptr::write_volatile(&raw mut (*h).fg_color, fg);
            core::ptr::write_volatile(&raw mut (*h).bg_color, bg);
            let prev = core::ptr::read_volatile(&(*h).init_count);
            core::ptr::write_volatile(&raw mut (*h).init_count, prev.wrapping_add(1));
            // Write magic last — this is the commit point.
            compiler_fence(Ordering::Release);
            core::ptr::write_volatile(&raw mut (*h).magic, HANDOFF_MAGIC);
        }
    }
}
