//! SDRAM initialisation for the IS42S32800J-6BLI on STM32H743I-EVAL.
//!
//! Uses the stm32-fmc `SdramChip` trait consumed by embassy-stm32's `Fmc`
//! peripheral.  After `init()` the SDRAM is memory-mapped at `0xD000_0000`.
//!
//! # Timing rationale
//!
//! The IS42S32800J-6BLI is a 6 ns speed-grade device.  The absolute maximum
//! allowed SDCLK is 1 / 6 ns ≈ 167 MHz.  Running at HCLK/2 = 200 MHz would
//! be *out of spec*.  **SDCLK must be HCLK/3 = 133 MHz** (7.5 ns/cycle).
//!
//! All timing constants below are given in nanoseconds; the `stm32-fmc` crate
//! converts them to clock cycles using the actual SDCLK frequency.
//!
//! ```text
//! At 7.5 ns/cycle:
//!   tRP  ≥ 18 ns  → 3 cycles
//!   tRCD ≥ 18 ns  → 3 cycles
//!   tRC  ≥ 60 ns  → 8 cycles
//!   tRAS ≥ 42 ns  → 6 cycles
//!   tXSR ≥ 67 ns  → 9 cycles
//!   tMRD ≥ 2 ck   → pass as 15 ns (2 × 7.5 ns)
//!   tWR  = 2 ck   → pass as 15 ns
//! ```
//!
//! Refresh: 8192 rows in 64 ms → 7812 ns per row.

use embassy_stm32::fmc::Fmc;
use embassy_stm32::peripherals::FMC;
use embassy_stm32::Peri;
use stm32_fmc::{SdramChip, SdramConfiguration, SdramTiming};

// ── IS42S32800J SdramChip implementation ─────────────────────────────────────

/// Chip descriptor for the IS42S32800J-6BLI (8M × 32-bit, 4 banks, Bank2).
pub struct Is42s32800j;

impl SdramChip for Is42s32800j {
    /// Mode register value:
    ///   Burst length = 1 (000), Sequential (0),
    ///   CAS latency = 3 (011), Standard mode (00),
    ///   Write burst = Single location (1) → bit 9 set.
    const MODE_REGISTER: u16 = 0x0230;

    const CONFIG: SdramConfiguration = SdramConfiguration {
        column_bits: 9, // A0–A8
        row_bits: 13,   // A0–A12
        memory_data_width: 32,
        internal_banks: 4,
        cas_latency: 3,
        write_protection: false,
        read_burst: true,
        read_pipe_delay_cycles: 0,
    };

    const TIMING: SdramTiming = SdramTiming {
        startup_delay_ns: 100_000,    // ≥ 100 µs power-up delay
        max_sd_clock_hz: 133_333_333, // enforce HCLK/3
        refresh_period_ns: 7_812,     // 64 ms / 8192 rows ≈ 7812 ns
        mode_register_to_active: 15,  // tMRD = 2 ck @ 7.5 ns = 15 ns
        exit_self_refresh: 67,        // tXSR ≥ 67 ns
        active_to_precharge: 42,      // tRAS ≥ 42 ns
        row_cycle: 60,                // tRC  ≥ 60 ns
        row_precharge: 18,            // tRP  ≥ 18 ns
        row_to_column: 18,            // tRCD ≥ 18 ns
    };
}

// ── Pin bundle ────────────────────────────────────────────────────────────────

/// All FMC pins for the IS42S32800J-6BLI on STM32H743I-EVAL (MB1246 Rev E).
///
/// Pin mapping from the MB1246 board schematic:
///
/// ```text
/// A0–A5    → PF0–PF5 (AF12)        A6–A9  → PF12–PF15 (AF12)
/// A10–A12  → PG0–PG2 (AF12)
/// BA0, BA1 → PG4, PG5 (AF12)
/// D0–D3    → PD14,PD15,PD0,PD1 (AF12)
/// D4–D15   → PE7–PE15, PD8–PD10 (AF12)
/// D16–D23  → PH8–PH15 (AF12)
/// D24–D27  → PI0–PI3 (AF12)
/// D28–D31  → PI6,PI7,PI9,PI10 (AF12)
/// NBL0,1   → PE0,PE1 (AF12)
/// NBL2,3   → PI4,PI5 (AF12)
/// SDCKE1   → PH7 (AF12)
/// SDCLK    → PG8 (AF12)
/// SDNCAS   → PG15 (AF12)
/// SDNE1    → PH6 (AF12)
/// SDNRAS   → PF11 (AF12)
/// SDNWE    → PH5 (AF12)
/// ```
#[allow(clippy::too_many_arguments)]
pub fn init_sdram<'d>(
    fmc: Peri<'d, FMC>,
    // Address A0–A12
    a0: Peri<'d, impl embassy_stm32::fmc::A0Pin<FMC>>,
    a1: Peri<'d, impl embassy_stm32::fmc::A1Pin<FMC>>,
    a2: Peri<'d, impl embassy_stm32::fmc::A2Pin<FMC>>,
    a3: Peri<'d, impl embassy_stm32::fmc::A3Pin<FMC>>,
    a4: Peri<'d, impl embassy_stm32::fmc::A4Pin<FMC>>,
    a5: Peri<'d, impl embassy_stm32::fmc::A5Pin<FMC>>,
    a6: Peri<'d, impl embassy_stm32::fmc::A6Pin<FMC>>,
    a7: Peri<'d, impl embassy_stm32::fmc::A7Pin<FMC>>,
    a8: Peri<'d, impl embassy_stm32::fmc::A8Pin<FMC>>,
    a9: Peri<'d, impl embassy_stm32::fmc::A9Pin<FMC>>,
    a10: Peri<'d, impl embassy_stm32::fmc::A10Pin<FMC>>,
    a11: Peri<'d, impl embassy_stm32::fmc::A11Pin<FMC>>,
    a12: Peri<'d, impl embassy_stm32::fmc::A12Pin<FMC>>,
    // Bank address BA0–BA1
    ba0: Peri<'d, impl embassy_stm32::fmc::BA0Pin<FMC>>,
    ba1: Peri<'d, impl embassy_stm32::fmc::BA1Pin<FMC>>,
    // Data D0–D31
    d0: Peri<'d, impl embassy_stm32::fmc::D0Pin<FMC>>,
    d1: Peri<'d, impl embassy_stm32::fmc::D1Pin<FMC>>,
    d2: Peri<'d, impl embassy_stm32::fmc::D2Pin<FMC>>,
    d3: Peri<'d, impl embassy_stm32::fmc::D3Pin<FMC>>,
    d4: Peri<'d, impl embassy_stm32::fmc::D4Pin<FMC>>,
    d5: Peri<'d, impl embassy_stm32::fmc::D5Pin<FMC>>,
    d6: Peri<'d, impl embassy_stm32::fmc::D6Pin<FMC>>,
    d7: Peri<'d, impl embassy_stm32::fmc::D7Pin<FMC>>,
    d8: Peri<'d, impl embassy_stm32::fmc::D8Pin<FMC>>,
    d9: Peri<'d, impl embassy_stm32::fmc::D9Pin<FMC>>,
    d10: Peri<'d, impl embassy_stm32::fmc::D10Pin<FMC>>,
    d11: Peri<'d, impl embassy_stm32::fmc::D11Pin<FMC>>,
    d12: Peri<'d, impl embassy_stm32::fmc::D12Pin<FMC>>,
    d13: Peri<'d, impl embassy_stm32::fmc::D13Pin<FMC>>,
    d14: Peri<'d, impl embassy_stm32::fmc::D14Pin<FMC>>,
    d15: Peri<'d, impl embassy_stm32::fmc::D15Pin<FMC>>,
    d16: Peri<'d, impl embassy_stm32::fmc::D16Pin<FMC>>,
    d17: Peri<'d, impl embassy_stm32::fmc::D17Pin<FMC>>,
    d18: Peri<'d, impl embassy_stm32::fmc::D18Pin<FMC>>,
    d19: Peri<'d, impl embassy_stm32::fmc::D19Pin<FMC>>,
    d20: Peri<'d, impl embassy_stm32::fmc::D20Pin<FMC>>,
    d21: Peri<'d, impl embassy_stm32::fmc::D21Pin<FMC>>,
    d22: Peri<'d, impl embassy_stm32::fmc::D22Pin<FMC>>,
    d23: Peri<'d, impl embassy_stm32::fmc::D23Pin<FMC>>,
    d24: Peri<'d, impl embassy_stm32::fmc::D24Pin<FMC>>,
    d25: Peri<'d, impl embassy_stm32::fmc::D25Pin<FMC>>,
    d26: Peri<'d, impl embassy_stm32::fmc::D26Pin<FMC>>,
    d27: Peri<'d, impl embassy_stm32::fmc::D27Pin<FMC>>,
    d28: Peri<'d, impl embassy_stm32::fmc::D28Pin<FMC>>,
    d29: Peri<'d, impl embassy_stm32::fmc::D29Pin<FMC>>,
    d30: Peri<'d, impl embassy_stm32::fmc::D30Pin<FMC>>,
    d31: Peri<'d, impl embassy_stm32::fmc::D31Pin<FMC>>,
    // Byte enable NBL0–NBL3
    nbl0: Peri<'d, impl embassy_stm32::fmc::NBL0Pin<FMC>>,
    nbl1: Peri<'d, impl embassy_stm32::fmc::NBL1Pin<FMC>>,
    nbl2: Peri<'d, impl embassy_stm32::fmc::NBL2Pin<FMC>>,
    nbl3: Peri<'d, impl embassy_stm32::fmc::NBL3Pin<FMC>>,
    // Control
    sdcke: Peri<'d, impl embassy_stm32::fmc::SDCKE1Pin<FMC>>,
    sdclk: Peri<'d, impl embassy_stm32::fmc::SDCLKPin<FMC>>,
    sdncas: Peri<'d, impl embassy_stm32::fmc::SDNCASPin<FMC>>,
    sdne: Peri<'d, impl embassy_stm32::fmc::SDNE1Pin<FMC>>,
    sdnras: Peri<'d, impl embassy_stm32::fmc::SDNRASPin<FMC>>,
    sdnwe: Peri<'d, impl embassy_stm32::fmc::SDNWEPin<FMC>>,
    // Delay source for power-up sequence
    delay: &mut impl embedded_hal::delay::DelayNs,
) -> *mut u16 {
    let mut sdram = Fmc::sdram_a13bits_d32bits_4banks_bank2(
        fmc,
        a0,
        a1,
        a2,
        a3,
        a4,
        a5,
        a6,
        a7,
        a8,
        a9,
        a10,
        a11,
        a12,
        ba0,
        ba1,
        d0,
        d1,
        d2,
        d3,
        d4,
        d5,
        d6,
        d7,
        d8,
        d9,
        d10,
        d11,
        d12,
        d13,
        d14,
        d15,
        d16,
        d17,
        d18,
        d19,
        d20,
        d21,
        d22,
        d23,
        d24,
        d25,
        d26,
        d27,
        d28,
        d29,
        d30,
        d31,
        nbl0,
        nbl1,
        nbl2,
        nbl3,
        sdcke,
        sdclk,
        sdncas,
        sdne,
        sdnras,
        sdnwe,
        Is42s32800j,
    );

    // Returns *mut u32 base pointer; cast to *mut u16 for RGB565 writes.
    let base_u32 = sdram.init(delay);
    base_u32 as *mut u16
}

/// SDRAM base address (FMC Bank 2).
pub const SDRAM_BASE: u32 = 0xD000_0000;

/// Size of one RGB565 framebuffer for a 640×480 display, in bytes.
pub const FB_SIZE_BYTES: usize = 640 * 480 * 2;

/// Number of u16 words in one framebuffer.
pub const FB_LEN: usize = 640 * 480;
