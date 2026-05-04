//! LTDC initialisation for the Ampire AM640480GTNQW display on CN20 of
//! the STM32H743I-EVAL board (MB1246 Rev E).
//!
//! # Panel specs (from official ST BSP `ampire640480.h`)
//!
//! | Parameter      | Value |
//! |---------------|-------|
//! | Pixel clock   | 25 MHz (PLL3R) |
//! | HSYNC width   | 30 cycles |
//! | H back porch  | 114 cycles |
//! | H front porch | 16 cycles |
//! | VSYNC height  | 3 lines |
//! | V back porch  | 32 lines |
//! | V front porch | 10 lines |
//! | HSYNC pol     | Active Low |
//! | VSYNC pol     | Active Low |
//! | DE pol        | Active Low |
//! | Pixel CLK pol | Falling Edge |
//!
//! # CN20 pin mapping (AF14 for all LTDC signals)
//!
//! ```text
//! CLK   → PI14   HSYNC → PI12   VSYNC → PI13   DE/ENB → PK7
//! R0 → PI15  R1 → PJ0   R2 → PJ1   R3 → PJ2   R4 → PJ3
//! R5 → PJ4   R6 → PJ5   R7 → PJ6
//! G0 → PJ7   G1 → PJ8   G2 → PJ9   G3 → PJ10  G4 → PJ11
//! G5 → PK0   G6 → PK1   G7 → PK2
//! B0 → PJ12  B1 → PJ13  B2 → PJ14  B3 → PJ15  B4 → PK3
//! B5 → PK4   B6 → PK5   B7 → PK6
//! BL_CTRL → PA6 (backlight, drive high to enable)
//! ```
//!
//! The DE pin (PK7 AF14) is configured separately because
//! `Ltdc::new_with_pins` doesn't accept a DE pin; the hardware drives it
//! automatically via the alternate function.

use embassy_stm32::gpio::{AfType, Flex, Level, Output, OutputType, Speed};
use embassy_stm32::ltdc::{
    Ltdc, LtdcConfiguration, LtdcLayer, LtdcLayerConfig, PixelFormat, PolarityActive, PolarityEdge,
};
use embassy_stm32::peripherals::LTDC;
use embassy_stm32::{interrupt, Peri};

/// Ampire 640×480 display timing constants.
pub mod ampire {
    pub const ACTIVE_W: u16 = 640;
    pub const ACTIVE_H: u16 = 480;
    pub const HSYNC: u16 = 30;
    pub const H_BACK_PORCH: u16 = 114;
    pub const H_FRONT_PORCH: u16 = 16;
    pub const VSYNC: u16 = 3;
    pub const V_BACK_PORCH: u16 = 32;
    pub const V_FRONT_PORCH: u16 = 10;
}

/// LTDC configuration for the Ampire AM640480GTNQW.
pub const AMPIRE_CONFIG: LtdcConfiguration = LtdcConfiguration {
    active_width: ampire::ACTIVE_W,
    active_height: ampire::ACTIVE_H,
    h_back_porch: ampire::H_BACK_PORCH,
    h_front_porch: ampire::H_FRONT_PORCH,
    h_sync: ampire::HSYNC,
    v_back_porch: ampire::V_BACK_PORCH,
    v_front_porch: ampire::V_FRONT_PORCH,
    v_sync: ampire::VSYNC,
    h_sync_polarity: PolarityActive::ActiveLow,
    v_sync_polarity: PolarityActive::ActiveLow,
    data_enable_polarity: PolarityActive::ActiveLow,
    pixel_clock_polarity: PolarityEdge::FallingEdge,
};

/// Layer configuration: full-screen RGB565, Layer1.
pub const LAYER1_CONFIG: LtdcLayerConfig = LtdcLayerConfig {
    layer: LtdcLayer::Layer1,
    pixel_format: PixelFormat::RGB565,
    window_x0: 0,
    window_x1: ampire::ACTIVE_W,
    window_y0: 0,
    window_y1: ampire::ACTIVE_H,
};

/// Initialise the LTDC peripheral for the Ampire CN20 display.
///
/// - Drives `PA6` high to enable the backlight.
/// - Configures `PK7` as LTDC DE output (AF14).
/// - Returns a ready-to-use [`Ltdc`] instance with Layer1 (RGB565) enabled.
///
/// The framebuffer address must be set immediately after return via
/// [`Ltdc::set_buffer`] or directly through the PAC register `LTDC_L1CFBAR`.
///
/// # Safety
/// `fb_addr` must point to a valid RGB565 framebuffer of at least
/// 640 × 480 × 2 = 614 400 bytes.
#[allow(clippy::too_many_arguments)]
pub fn init_ltdc<'d>(
    peri: Peri<'d, LTDC>,
    irq: impl interrupt::typelevel::Binding<
            interrupt::typelevel::LTDC,
            embassy_stm32::ltdc::InterruptHandler<LTDC>,
        > + 'd,
    // Backlight control
    bl_ctrl: Peri<'d, impl embassy_stm32::gpio::Pin>,
    // DE pin — configured as AF14 and forgotten (HW drives it)
    de: Peri<'d, impl embassy_stm32::ltdc::DePin<LTDC>>,
    // CLK, HSYNC, VSYNC
    clk: Peri<'d, impl embassy_stm32::ltdc::ClkPin<LTDC>>,
    hsync: Peri<'d, impl embassy_stm32::ltdc::HsyncPin<LTDC>>,
    vsync: Peri<'d, impl embassy_stm32::ltdc::VsyncPin<LTDC>>,
    // Red channel R0–R7
    r0: Peri<'d, impl embassy_stm32::ltdc::R0Pin<LTDC>>,
    r1: Peri<'d, impl embassy_stm32::ltdc::R1Pin<LTDC>>,
    r2: Peri<'d, impl embassy_stm32::ltdc::R2Pin<LTDC>>,
    r3: Peri<'d, impl embassy_stm32::ltdc::R3Pin<LTDC>>,
    r4: Peri<'d, impl embassy_stm32::ltdc::R4Pin<LTDC>>,
    r5: Peri<'d, impl embassy_stm32::ltdc::R5Pin<LTDC>>,
    r6: Peri<'d, impl embassy_stm32::ltdc::R6Pin<LTDC>>,
    r7: Peri<'d, impl embassy_stm32::ltdc::R7Pin<LTDC>>,
    // Green channel G0–G7
    g0: Peri<'d, impl embassy_stm32::ltdc::G0Pin<LTDC>>,
    g1: Peri<'d, impl embassy_stm32::ltdc::G1Pin<LTDC>>,
    g2: Peri<'d, impl embassy_stm32::ltdc::G2Pin<LTDC>>,
    g3: Peri<'d, impl embassy_stm32::ltdc::G3Pin<LTDC>>,
    g4: Peri<'d, impl embassy_stm32::ltdc::G4Pin<LTDC>>,
    g5: Peri<'d, impl embassy_stm32::ltdc::G5Pin<LTDC>>,
    g6: Peri<'d, impl embassy_stm32::ltdc::G6Pin<LTDC>>,
    g7: Peri<'d, impl embassy_stm32::ltdc::G7Pin<LTDC>>,
    // Blue channel B0–B7
    b0: Peri<'d, impl embassy_stm32::ltdc::B0Pin<LTDC>>,
    b1: Peri<'d, impl embassy_stm32::ltdc::B1Pin<LTDC>>,
    b2: Peri<'d, impl embassy_stm32::ltdc::B2Pin<LTDC>>,
    b3: Peri<'d, impl embassy_stm32::ltdc::B3Pin<LTDC>>,
    b4: Peri<'d, impl embassy_stm32::ltdc::B4Pin<LTDC>>,
    b5: Peri<'d, impl embassy_stm32::ltdc::B5Pin<LTDC>>,
    b6: Peri<'d, impl embassy_stm32::ltdc::B6Pin<LTDC>>,
    b7: Peri<'d, impl embassy_stm32::ltdc::B7Pin<LTDC>>,
    // Initial framebuffer base address (in SDRAM)
    fb_addr: u32,
) -> Ltdc<'d, LTDC> {
    // ── Backlight on ─────────────────────────────────────────────────────────
    let _bl = Output::new(bl_ctrl, Level::High, Speed::Low);
    core::mem::forget(_bl); // keep high forever, release ownership

    // ── Configure DE pin (PK7, AF14) ─────────────────────────────────────────
    // embassy-stm32's Ltdc::new_with_pins does not include DE; configure it
    // manually.  The LTDC peripheral drives PK7 as AF14 automatically once
    // the peripheral is enabled.
    let mut de_flex = Flex::new(de);
    de_flex.set_as_af_unchecked(14, AfType::output(OutputType::PushPull, Speed::VeryHigh));
    core::mem::forget(de_flex); // LTDC hardware owns the pin now

    // ── LTDC peripheral ──────────────────────────────────────────────────────
    let mut ltdc = Ltdc::new_with_pins(
        peri, irq, clk, hsync, vsync, b0, b1, b2, b3, b4, b5, b6, b7, g0, g1, g2, g3, g4, g5, g6,
        g7, r0, r1, r2, r3, r4, r5, r6, r7,
    );

    ltdc.init(&AMPIRE_CONFIG);
    ltdc.init_layer(&LAYER1_CONFIG, None);

    // Set the initial framebuffer address directly via PAC (no async needed
    // at init time — LTDC is not yet scanning; immediate reload is safe).
    use embassy_stm32::pac;
    pac::LTDC.layer(0).cfbar().write(|w| w.set_cfbadd(fb_addr));
    // Immediate reload (not VBR) so the address takes effect right away.
    pac::LTDC
        .srcr()
        .write(|w| w.set_imr(embassy_stm32::pac::ltdc::vals::Imr::RELOAD));

    ltdc
}
