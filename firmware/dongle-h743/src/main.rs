//! KCAN dongle firmware — STM32H743XI (STM32H743I-EVAL, MB1246 Rev E).
//!
//! # Architecture
//!
//! ```text
//! USB Bulk OUT (host→device)
//!     │  usb_task reads 80-byte KCanFrame
//!     ▼
//! usb_to_can: Channel<KCanFrame, 32>
//!     │  can_task dequeues and writes to FDCAN1
//!     ▼
//! FDCAN1 TX ─────────────────────────── CAN bus ──────────────────── FDCAN1 RX
//!                                                                         │
//!                                                             FDCAN1 RX ISR
//!                                                                         │ snapshot TIM2
//!                                                                         ▼
//!                                                          can_to_usb: Channel<KCanFrame, 32>
//!                                                                         │
//!                                                              usb_task writes Bulk IN
//!                                                                         ▼
//!                                                         USB Bulk IN (device→host)
//! ```
//!
//! # Pins (STM32H743I-EVAL, MB1246 Rev E)
//!
//! | Signal        | Pin  | Notes                                           |
//! |---------------|------|-------------------------------------------------|
//! | FDCAN1_RX     | PA11 | On-board TJA1044 transceiver → CN3 DB9 (ch 0)  |
//! | FDCAN1_TX     | PA12 | On-board TJA1044 transceiver → CN3 DB9 (ch 0)  |
//! | ULPI_CLK      | PA5  | CN14 USB-HS — USB3320C-EZK (25 MHz ref → 60 MHz ULPI out) |
//! | ULPI_STP      | PC0  | CN14 USB-HS                                     |
//! | ULPI_DIR      | PI11 | CN14 USB-HS                                     |
//! | ULPI_NXT      | PH4  | CN14 USB-HS                                     |
//! | ULPI_D0       | PA3  | CN14 USB-HS data bus                            |
//! | ULPI_D1       | PB0  | CN14 USB-HS data bus                            |
//! | ULPI_D2       | PB1  | CN14 USB-HS data bus                            |
//! | ULPI_D3       | PB10 | CN14 USB-HS data bus                            |
//! | ULPI_D4       | PB11 | CN14 USB-HS data bus                            |
//! | ULPI_D5       | PB12 | CN14 USB-HS data bus                            |
//! | ULPI_D6       | PB13 | CN14 USB-HS data bus                            |
//! | ULPI_D7       | PB5  | CN14 USB-HS data bus                            |
//! | LD1 (green)   | PF10 | Heartbeat — 1 Hz blink = firmware alive         |
//! | LD3 (orange)  | PA4  | USB host connected (solid on = enumerated)      |
//!
//! # Clocks
//!
//! System clock: 400 MHz (HSE 25 MHz + PLL1).
//! FDCAN kernel clock: 32 MHz (PLL2Q = 320 MHz / 10).
//! TIM2: 1 MHz free-running counter for hardware timestamps.
//! USB OTG HS: external 60 MHz clock from USB3320C-EZK ULPI PHY on CN14.
//!
//! # PLL derivation
//!
//! PLL1 (sysclk): 25 MHz / 5 = 5 MHz → 5 × 192 = 960 MHz VCO → / 2 = 480 MHz
//! PLL2 (FDCAN):  25 MHz / 5 = 5 MHz → 5 × 64  = 320 MHz VCO → / 10 = 32 MHz

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_stm32::bind_interrupts;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::usb::Driver;
use embassy_stm32::Config;
use embassy_stm32::{can, peripherals, usb};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_usb::msos::{self, windows_version};
use embassy_usb::Builder as UsbBuilder;

use defmt::*;
use defmt_rtt as _;
use panic_probe as _;

// ─── HardFault recovery ───────────────────────────────────────────────────────
// The Embassy USB OTG HS / ULPI reinit path faults when Windows issues
// rapid consecutive bus resets during first-time WinUSB driver installation
// (4 resets in <100 ms, addresses 8→9→10→11 visible in RTT trace).
// panic_probe's default behaviour is to halt the CPU, leaving the ULPI PHY
// stuck and Windows permanently seeing a bad descriptor (Code 43).
// Resetting via SCB lets the device re-enumerate cleanly on the next attempt;
// Windows uses the already-cached WinUSB binding and issues only 1-2 resets.
#[cortex_m_rt::exception]
unsafe fn HardFault(_ef: &cortex_m_rt::ExceptionFrame) -> ! {
    defmt::error!("HardFault — performing system reset for clean re-enumeration");
    cortex_m::peripheral::SCB::sys_reset()
}

mod can_task;
mod dfu_app;
mod display_task;
#[cfg(feature = "periodic-echo")]
mod echo_task;
mod ep0_handler;
mod kcan_usb;
mod status_task;
mod usb_task;

extern crate lcd_terminal;

use kcan_protocol::frame::KCanFrame;
use kcan_usb::KCanUsbClass;

// ─── Shared channels ──────────────────────────────────────────────────────────

/// CAN RX → USB Bulk IN.
static CAN_TO_USB: Channel<CriticalSectionRawMutex, KCanFrame, 32> = Channel::new();

/// USB Bulk OUT → FDCAN1 TX (channel 0).
static USB_TO_CAN: Channel<CriticalSectionRawMutex, KCanFrame, 32> = Channel::new();

/// Dummy second-channel sink required by kcan_io_task signature.
/// No task reads from this; channel-1 frames from the host are silently queued.
static USB_TO_CAN2: Channel<CriticalSectionRawMutex, KCanFrame, 32> = Channel::new();

// ─── Interrupt bindings ───────────────────────────────────────────────────────

bind_interrupts!(struct Irqs {
    OTG_HS     => usb::InterruptHandler<peripherals::USB_OTG_HS>;
    FDCAN1_IT0 => can::IT0InterruptHandler<peripherals::FDCAN1>;
    FDCAN1_IT1 => can::IT1InterruptHandler<peripherals::FDCAN1>;
    LTDC       => embassy_stm32::ltdc::InterruptHandler<peripherals::LTDC>;
});

// ─── Entry point ─────────────────────────────────────────────────────────────

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // ── D-Cache disable (USB DMA coherency) ──────────────────────────────────
    // The STM32H7 L1 D-cache causes USB DMA vs CPU coherency issues: the USB
    // OTG DMA writes SETUP/OUT packets into SRAM but the CPU reads stale cache
    // lines and never sees the data, leaving the stack stuck at "SETUP waiting".
    // Disabling the D-cache is the simplest fix; a non-cacheable MPU region is
    // the production alternative once USB is verified working.
    unsafe {
        let mut cp = cortex_m::peripheral::Peripherals::steal();
        cp.SCB.disable_dcache(&mut cp.CPUID);
    }

    // ── Clock configuration ───────────────────────────────────────────────────
    // System:  HSE 25 MHz → PLL1 → 400 MHz sysclk  (VoltageScale1)
    // FDCAN:   HSE 25 MHz → PLL2Q → 32 MHz
    // USB:     HSI48 with CRS (simplest, no PLL3 needed)
    let mut config = Config::default();
    {
        use embassy_stm32::rcc::*;
        config.rcc.hse = Some(Hse {
            freq: embassy_stm32::time::Hertz(25_000_000),
            mode: HseMode::Oscillator,
        });
        // PLL1: 25 MHz / 5 = 5 MHz → 5 × 160 = 800 MHz VCO → / 2 = 400 MHz sysclk.
        // Using 400 MHz (VoltageScale1) instead of 480 MHz (Scale0) to avoid
        // STM32H7 power-supply instability known to break USB enumeration.
        config.rcc.pll1 = Some(Pll {
            source: PllSource::HSE,
            prediv: PllPreDiv::DIV5,  // → 5 MHz
            mul: PllMul::MUL160,      // → 800 MHz VCO
            divp: Some(PllDiv::DIV2), // → 400 MHz sysclk
            divq: None,
            divr: None,
            fracn: None,
        });
        // PLL2: 25 MHz / 5 = 5 MHz → 5 × 64 = 320 MHz VCO → / 10 = 32 MHz FDCAN clock.
        config.rcc.pll2 = Some(Pll {
            source: PllSource::HSE,
            prediv: PllPreDiv::DIV5, // → 5 MHz
            mul: PllMul::MUL64,      // → 320 MHz VCO
            divp: None,
            divq: Some(PllDiv::DIV10), // → 32 MHz
            divr: None,
            fracn: None,
        });
        config.rcc.sys = Sysclk::PLL1_P;
        config.rcc.ahb_pre = AHBPrescaler::DIV2; // 200 MHz HCLK
        config.rcc.apb1_pre = APBPrescaler::DIV2; // 100 MHz
        config.rcc.apb2_pre = APBPrescaler::DIV2; // 100 MHz
        config.rcc.apb3_pre = APBPrescaler::DIV2; // 100 MHz
        config.rcc.apb4_pre = APBPrescaler::DIV2; // 100 MHz
        config.rcc.voltage_scale = VoltageScale::Scale1;
        // CSI required by STM32H7 USB analog circuits (see embassy USB examples).
        config.rcc.csi = true;
        // FDCAN kernel clock from PLL2Q.
        config.rcc.mux.fdcansel = mux::Fdcansel::PLL2_Q;
        // USB clock from HSI48 (with CRS).
        config.rcc.hsi48 = Some(Hsi48Config {
            sync_from_usb: false, // ULPI PHY (USB3320C-EZK) provides its own 60 MHz clock
        });
        config.rcc.mux.usbsel = mux::Usbsel::HSI48;

        // PLL3: pixel clock for LTDC → 25 MHz
        //   HSE 25 MHz / M=5 = 5 MHz ref
        //   5 MHz × N=160 = 800 MHz VCO
        //   800 MHz / R=32 = 25 MHz → LTDCCLK
        config.rcc.pll3 = Some(Pll {
            source: PllSource::HSE,
            prediv: PllPreDiv::DIV5, // → 5 MHz
            mul: PllMul::MUL160,     // → 800 MHz VCO
            divp: None,
            divq: None,
            divr: Some(PllDiv::DIV32), // → 25 MHz LTDCCLK
            fracn: None,
        });
        // PLL3R is the only LTDC clock source on STM32H743 — no mux to set.
    }

    let p = embassy_stm32::init(config);

    // ── STM32H7 unique device ID ───────────────────────────────────────────────
    // 96-bit UID at 0x1FF1_E800 (STM32H7 reference manual §60.1).
    // Lower 32 bits are used as a short unique identifier.
    // SAFETY: read-only UID register, no aliasing.
    let uid_lo = unsafe { core::ptr::read_volatile(0x1FF1_E800u32 as *const u32) };

    // Format as 8-char uppercase hex stored in a 'static buffer so it can be
    // passed as a &'static str to the USB serial number descriptor.
    static mut UID_SERIAL_BUF: [u8; 8] = *b"00000000";
    // SAFETY: main() runs once; no concurrent access before tasks are spawned.
    let serial_str: &'static str = unsafe {
        let buf = &mut *core::ptr::addr_of_mut!(UID_SERIAL_BUF);
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        for i in 0..8usize {
            buf[7 - i] = HEX[((uid_lo >> (i * 4)) & 0xF) as usize];
        }
        core::str::from_utf8_unchecked(buf)
    };

    info!("KCAN Dongle v1 (H743I) — booting  uid={:08X}", uid_lo);
    info!("USB: VID=0x1209 PID=0xBEEF  serial={}", serial_str);

    // ── Status LEDs ───────────────────────────────────────────────────────────
    // MB1246 Rev E direct-GPIO LEDs:
    //   LD1 (green)  → PF10  — heartbeat
    //   LD3 (orange) → PA4   — USB host connected
    let led_heartbeat = Output::new(p.PF10, Level::Low, Speed::Low);
    let led_usb = Output::new(p.PA4, Level::Low, Speed::Low);

    // ── FDCAN1: channel 0 — on-board TJA1044 → CN3 DB9 (PA11/PA12) ──────────
    let mut can1_cfg = can::CanConfigurator::new(p.FDCAN1, p.PA11, p.PA12, Irqs);
    can1_cfg.set_bitrate(250_000);
    #[cfg(not(feature = "loopback"))]
    let can = {
        let c = can1_cfg.into_normal_mode();
        info!("FDCAN1: normal mode, 250 kbps (PLL2Q = 32 MHz) — CN3 DB9");
        c
    };
    #[cfg(feature = "loopback")]
    let can = {
        let c = can1_cfg.into_internal_loopback_mode();
        info!("FDCAN1: INTERNAL LOOPBACK mode, 250 kbps — Phase 2 self-test");
        c
    };

    // ── USB OTG HS — CN14 ULPI (USB3320C-EZK) ────────────────────────────────
    {
        use embassy_stm32::pac;

        // Enable OTG_HS AHB1 clock and ULPI clock gate.
        // The ULPI gate must be on before Driver::new_hs_ulpi() configures the
        // PHY; Embassy's Bus::init() also enables it, but doing it here avoids
        // any race between clock enable and the ULPI pin config macro.
        pac::RCC.ahb1enr().modify(|w| {
            w.set_usb_otg_hsen(true);
            w.set_usb_otg_hs_ulpien(true);
            w.set_usb_otg_fsen(false); // gate off unused FS peripheral
        });
        let _ = pac::RCC.ahb1enr().read(); // barrier

        // Enable USB 3.3 V supply detector — required for OTG_HS analog circuits.
        pac::PWR.cr3().modify(|w| {
            w.set_usb33den(true);
            w.set_usbregen(false);
        });
        while !pac::PWR.cr3().read().usb33rdy() {}
        info!("USB: VDD33USB ready (usb33rdy=1)");
    }

    // ep_out_buffer must be 'static for the driver lifetime.
    // 1024 bytes for OTG_HS FIFO (OTG_FS only needed 256).
    static mut USB_EP_OUT_BUF: [u8; 1024] = [0u8; 1024];
    let mut usb_cfg = embassy_stm32::usb::Config::default();
    usb_cfg.vbus_detection = false;
    // SAFETY: main is only ever called once by the embassy runtime.
    // Parameter order: peri, irq, clk, dir, nxt, stp, d0..d7, buf, config.
    let usb_driver = Driver::new_hs_ulpi(
        p.USB_OTG_HS,
        Irqs,
        p.PA5,  // ULPI_CLK
        p.PI11, // ULPI_DIR
        p.PH4,  // ULPI_NXT
        p.PC0,  // ULPI_STP
        p.PA3,  // ULPI_D0
        p.PB0,  // ULPI_D1
        p.PB1,  // ULPI_D2
        p.PB10, // ULPI_D3
        p.PB11, // ULPI_D4
        p.PB12, // ULPI_D5
        p.PB13, // ULPI_D6
        p.PB5,  // ULPI_D7
        unsafe { &mut *core::ptr::addr_of_mut!(USB_EP_OUT_BUF) },
        usb_cfg,
    );

    // ── USB device descriptor ─────────────────────────────────────────────────
    static mut CONFIG_DESCRIPTOR: [u8; 256] = [0u8; 256];
    static mut BOS_DESCRIPTOR: [u8; 256] = [0u8; 256];
    // MSOS 2.0 descriptor buffer — holds the WinUSB CompatibleID feature and
    // DeviceInterfaceGUID property written by builder.msos_feature() below.
    static mut MSOS_DESCRIPTOR: [u8; 256] = [0u8; 256];
    static mut CONTROL_BUF: [u8; 64] = [0u8; 64];

    let mut usb_config = embassy_usb::Config::new(0x1209, 0xBEEF);
    usb_config.manufacturer = Some("Kodezine");
    usb_config.product = Some("KCAN Dongle v1 (H743I)");
    usb_config.serial_number = Some(serial_str);
    // Single vendor-class interface — no IAD needed.
    usb_config.composite_with_iads = false;
    // bDeviceClass=0x00: class defined at interface level.
    // This causes macOS IOKit to create IOUSBHostInterface child nodes
    // (required for nusb to claim the interface).  The interface itself
    // is still vendor-specific (0xFF/0xFF/0xFF).
    usb_config.device_class = 0x00;
    usb_config.device_sub_class = 0x00;
    usb_config.device_protocol = 0x00;

    // SAFETY: static mut buffers initialised once here before any tasks start.
    let mut builder = UsbBuilder::new(
        usb_driver,
        usb_config,
        unsafe { &mut *core::ptr::addr_of_mut!(CONFIG_DESCRIPTOR) },
        unsafe { &mut *core::ptr::addr_of_mut!(BOS_DESCRIPTOR) },
        unsafe { &mut *core::ptr::addr_of_mut!(MSOS_DESCRIPTOR) },
        unsafe { &mut *core::ptr::addr_of_mut!(CONTROL_BUF) },
    );

    // EP0 vendor handler: responds to GET_INFO / GET_BT_CONST and ACKs
    // SET_BITTIMING / SET_MODE; also drives USB_CONFIGURED signal.
    static mut EP0_HANDLER: ep0_handler::KCanEp0Handler = ep0_handler::KCanEp0Handler { uid_lo: 0 };
    // SAFETY: single-threaded init before tasks are spawned.
    let ep0 = unsafe { &mut *core::ptr::addr_of_mut!(EP0_HANDLER) };
    ep0.uid_lo = uid_lo;
    builder.handler(ep0);

    // ── Microsoft OS 2.0 descriptor (WCID) ───────────────────────────────────
    // Tells Windows to automatically bind the WinUSB driver on first plug-in
    // without requiring Zadig or any manual driver installation.
    //
    // Vendor code 0x20 is used for the MSOS GET_DESCRIPTOR request issued by
    // Windows during enumeration.  It must not collide with KCAN EP0 request
    // codes (0x01-0x06, 0x10, 0x11).
    //
    // GUID {3FA8AAA5-7EFE-4CC1-9D09-94D04DE7049F} is shared between dongle-h743
    // and dongle-h753 (same VID/PID = same logical device class to Windows).
    //
    // macOS and Linux ignore BOS capabilities they do not recognise; no
    // regression on those platforms.
    builder.msos_descriptor(windows_version::WIN8_1, 0x20);
    builder.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
    builder.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
        "DeviceInterfaceGUIDs",
        msos::PropertyData::RegMultiSz(&["{3FA8AAA5-7EFE-4CC1-9D09-94D04DE7049F}"]),
    ));

    // ── KCAN USB class ───────────────────────────────────────────────────────
    let kcan_class = KCanUsbClass::new(&mut builder);

    // ── DFU Runtime interface ────────────────────────────────────────────────
    // Exposes a DFU Runtime interface (class 0xFE/01/01) so the host can
    // trigger firmware update mode via a standard DFU_DETACH + USB reset
    // sequence.  The handler signals dfu_app_task which writes flash + resets.
    static mut DFU_STATE: Option<dfu_app::DfuState<dfu_app::AppDfuHandler>> = None;
    // SAFETY: single-threaded init, no concurrent access before builder.build().
    let dfu_state = unsafe {
        DFU_STATE = Some(dfu_app::make_dfu_state(dfu_app::AppDfuHandler));
        (&raw mut DFU_STATE).as_mut().unwrap().as_mut().unwrap()
    };
    embassy_usb_dfu::application::usb_dfu(&mut builder, dfu_state, |_| {});

    let usb = builder.build();

    // ── Spawn tasks ───────────────────────────────────────────────────────────
    spawner.spawn(usb_task::usb_device_task(usb).ok().unwrap());
    spawner.spawn(
        usb_task::kcan_io_task(kcan_class, &CAN_TO_USB, &USB_TO_CAN, &USB_TO_CAN2)
            .ok()
            .unwrap(),
    );
    spawner.spawn(
        can_task::can_task(can, &CAN_TO_USB, &USB_TO_CAN)
            .ok()
            .unwrap(),
    );
    spawner.spawn(
        status_task::status_task(led_heartbeat, led_usb, &CAN_TO_USB)
            .ok()
            .unwrap(),
    );
    spawner.spawn(dfu_app::dfu_app_task(p.FLASH).ok().unwrap());
    #[cfg(feature = "periodic-echo")]
    spawner.spawn(echo_task::echo_task(&USB_TO_CAN).ok().unwrap());

    // ── LCD terminal (CN20 Ampire AM640480GTNQW on STM32H743I-EVAL) ───────────
    // Provide a simple blocking delay for the SDRAM 100 µs power-up sequence.
    struct BlockingDelay;
    impl embedded_hal::delay::DelayNs for BlockingDelay {
        fn delay_ns(&mut self, ns: u32) {
            // At 400 MHz sysclk, 1 ns ≈ 0.4 cycles; conservative NOP spin.
            let cycles = (ns as u64 * 400).div_ceil(1000);
            for _ in 0..cycles {
                cortex_m::asm::nop();
            }
        }
    }

    // ── SDRAM init ────────────────────────────────────────────────────────────
    // Returns *mut u16 base at 0xD000_0000 (SDCLK = HCLK/3 = 133 MHz enforced
    // by Is42s32800j::TIMING.max_sd_clock_hz in lcd_terminal::sdram).
    let fb: *mut u16 = lcd_terminal::sdram::init_sdram(
        p.FMC,
        // Address A0–A12: PF0–PF5, PF12–PF15, PG0–PG2
        p.PF0,
        p.PF1,
        p.PF2,
        p.PF3,
        p.PF4,
        p.PF5,
        p.PF12,
        p.PF13,
        p.PF14,
        p.PF15,
        p.PG0,
        p.PG1,
        p.PG2,
        // Bank address BA0, BA1: PG4, PG5
        p.PG4,
        p.PG5,
        // Data D0–D31
        p.PD14,
        p.PD15,
        p.PD0,
        p.PD1,
        p.PE7,
        p.PE8,
        p.PE9,
        p.PE10,
        p.PE11,
        p.PE12,
        p.PE13,
        p.PE14,
        p.PE15,
        p.PD8,
        p.PD9,
        p.PD10,
        p.PH8,
        p.PH9,
        p.PH10,
        p.PH11,
        p.PH12,
        p.PH13,
        p.PH14,
        p.PH15,
        p.PI0,
        p.PI1,
        p.PI2,
        p.PI3,
        p.PI6,
        p.PI7,
        p.PI9,
        p.PI10,
        // Byte enables NBL0–NBL3: PE0, PE1, PI4, PI5
        p.PE0,
        p.PE1,
        p.PI4,
        p.PI5,
        // SDRAM control (Bank2): SDCKE1=PH7, SDCLK=PG8, SDNCAS=PG15,
        //                        SDNE1=PH6, SDNRAS=PF11, SDNWE=PH5
        p.PH7,
        p.PG8,
        p.PG15,
        p.PH6,
        p.PF11,
        p.PH5,
        &mut BlockingDelay,
    );

    // ── LTDC + console ────────────────────────────────────────────────────────
    // SAFETY: `fb` points to the SDRAM framebuffer (614 400 bytes, non-cached,
    // static lifetime) initialised above by `sdram::init_sdram`.
    let lcd = unsafe {
        lcd_terminal::init_or_attach(
            fb, p.LTDC, Irqs, p.PA6, // BL_CTRL (backlight enable, active high)
            p.PK7, // DE/ENB (AF14 — driven by LTDC hardware)
            // CLK, HSYNC, VSYNC
            p.PI14, p.PI12, p.PI13, // Red R0–R7
            p.PI15, p.PJ0, p.PJ1, p.PJ2, p.PJ3, p.PJ4, p.PJ5, p.PJ6, // Green G0–G7
            p.PJ7, p.PJ8, p.PJ9, p.PJ10, p.PJ11, p.PK0, p.PK1, p.PK2, // Blue B0–B7
            p.PJ12, p.PJ13, p.PJ14, p.PJ15, p.PK3, p.PK4, p.PK5, p.PK6,
        )
    };

    // Read bootloader version from RTC backup registers (written by bootloader
    // before jumping to the app).  Sentinel 0xB007_B007 in BKP2R confirms a
    // valid bootloader handoff; BKP1R holds the packed version (maj<<16|min<<8|pat).
    // If the app runs without a bootloader (e.g. directly via probe-rs), the
    // sentinel is absent and bl_version is None — no BL line on the LCD.
    let bl_version: Option<(u8, u8, u8)> = {
        use embassy_stm32::pac;
        if pac::RTC.bkpr(2).read().bkp() == 0xB007_B007 {
            let p = pac::RTC.bkpr(1).read().bkp();
            Some(((p >> 16) as u8, (p >> 8) as u8, p as u8))
        } else {
            None
        }
    };

    spawner.spawn(display_task::display_task(lcd, bl_version).ok().unwrap());

    info!("All tasks spawned — ready for USB enumeration");
}
