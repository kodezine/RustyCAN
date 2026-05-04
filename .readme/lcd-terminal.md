# LCD Boot Terminal

A scrolling dmesg-style boot console on the 5.7" Ampire AM640480GTNQW TFT display
connected to the STM32H743I-EVAL board (MB1246 Rev E) via the CN20 50-pin LCD connector.

## Hardware

| Component | Part | Notes |
|-----------|------|-------|
| Display | Ampire AM640480GTNQW | 640×480, RGB parallel, 5.7" |
| Controller | STM32H743XI LTDC | 24-bit parallel RGB, clocked at 25 MHz |
| Frame buffer | IS42S32800J-6BLI SDRAM | 32 MB × 32-bit on FMC Bank 2 |
| Pixel format | RGB565 | 2 bytes/pixel → 614 400 bytes/frame |
| Font | IBM CP437 8×16 VGA | 80 cols × 30 rows = 2400 cells |
| Acceleration | DMA2D Chrom-ART | fill / scroll hardware-accelerated |

## CN20 Pin Mapping (AF14 for all LTDC signals)

| Signal | MCU Pin | Signal | MCU Pin |
|--------|---------|--------|---------|
| CLK    | PI14    | ENB/DE | PK7     |
| HSYNC  | PI12    | VSYNC  | PI13    |
| R0–R7  | PI15, PJ0–PJ6 | G0–G7 | PJ7–PJ11, PK0–PK2 |
| B0–B7  | PJ12–PJ15, PK3–PK6 | BL_CTRL | PA6 |

## Clock Configuration

PLL3 generates the 25 MHz LTDC pixel clock:

```
HSE 25 MHz → M=5 → 5 MHz → N=160 → 800 MHz VCO → R=32 → 25 MHz (PLL3R)
```

RCC_D1CCIPR LTDCCLKSEL is set to PLL3R by embassy-stm32 when
`config.rcc.mux.ltdcsel = mux::Ltdcsel::PLL3_R`.

**SDRAM timing constraint:** The IS42S32800J-6BLI is a 6 ns speed-grade part;
maximum SDCLK = 167 MHz. The FMC is programmed with SDCLK = HCLK/3 = 133 MHz
(7.5 ns/cycle). Running at HCLK/2 = 200 MHz is **out of spec** and must not be used.

## Warm Boot Handoff

The `LcdHandoff` struct is stored in a `.lcd_handoff` NOLOAD linker section in
SRAM4 (0x38000000). `cortex-m-rt` never zeroes NOLOAD sections, so the struct
survives a software reset / bootloader→firmware CPU jump.

| Field | Type | Meaning |
|-------|------|---------|
| `magic` | u32 | 0xCAFE_FEED = valid handoff |
| `fb_addr` | u32 | SDRAM framebuffer base address |
| `cursor_row` | u8 | Current text row (0–29) |
| `cursor_col` | u8 | Current text column (0–79) |
| `fg_color` | u16 | Current foreground colour (RGB565) |
| `bg_color` | u16 | Current background colour (RGB565) |
| `init_count` | u32 | Number of successful hardware inits |

## Shared Crate

`firmware/lcd-terminal/` is an independent `no_std` crate that can be added as
a dependency by any STM32H7 firmware in this workspace:

```toml
# firmware/<your-crate>/Cargo.toml
[dependencies]
lcd-terminal = { path = "../../lcd-terminal" }
```

The crate's `build.rs` automatically injects `lcd_handoff.x` into the linker,
placing the `.lcd_handoff` NOLOAD section in SRAM4.

### API

```rust
// In main():
let lcd = lcd_terminal::init_or_attach(
    p.LTDC, irqs,
    p.PA6,  // BL_CTRL
    p.PK7,  // DE
    /* CLK, HSYNC, VSYNC, R0–R7, G0–G7, B0–B7 */
    sdram_pins,
    &mut delay,
);
spawner.spawn(display_task(lcd)).unwrap();

// From any task:
use lcd_terminal::{boot_log, BootStatus};
boot_log!(LOG_CHANNEL, "FDCAN1 ready", BootStatus::Ok);
```

### Colour Palette (RGB565)

| Name | Value | Use |
|------|-------|-----|
| `BG_BLACK` | 0x0000 | Background |
| `FG_WHITE` | 0xFFFF | Default text |
| `DIM_GREEN` | 0x03E0 | Timestamps |
| `BRIGHT_GREEN` | 0x07E0 | `[  OK  ]` |
| `BRIGHT_RED` | 0xF800 | `[FAILED]` |
| `BRIGHT_YELLOW` | 0xFFE0 | `[ WARN ]` |
| `CYAN` | 0x07FF | Banner / info |

## Boot Log Format

```
[SSSSS.uuuuuu] message text                              [  OK  ]
```

- Timestamp (dim-green, 15 chars)
- Message (up to 56 chars, white)
- Status tag at column 72 (8 chars, colour-coded)
