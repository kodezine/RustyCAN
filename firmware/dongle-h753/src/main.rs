//! KCAN dongle firmware — STM32H753ZI.
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
//! # Pins (Nucleo-H753ZI)
//!
//! | Signal        | Pin  | Notes                              |
//! |---------------|------|------------------------------------|
//! | FDCAN1_RX     | PD0  | Via onboard TJA1051T transceiver   |
//! | FDCAN1_TX     | PD1  | Via onboard TJA1051T transceiver   |
//! | USB_OTG_FS_DM | PA11 | Micro-B connector CN5              |
//! | USB_OTG_FS_DP | PA12 | Micro-B connector CN5              |
//! | LD1 (green)   | PB0  | Bus-on indicator                   |
//! | LD2 (blue)    | PE1  | RX frame blink                     |
//! | LD3 (red)     | PB14 | TX error blink                     |
//!
//! # Clocks
//!
//! System clock: 480 MHz (HSE + PLL1).
//! FDCAN kernel clock: 32 MHz (PLL2Q = 64 MHz / 2, configured below).
//! TIM2: 1 MHz free-running counter for hardware timestamps.
//! USB OTG FS: 48 MHz (PLL3Q).

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
use embassy_usb::Builder as UsbBuilder;

use defmt::*;
use defmt_rtt as _;
use panic_probe as _;

mod can_task;
mod kcan_usb;
mod status_task;
mod usb_task;

use kcan_protocol::frame::KCanFrame;
use kcan_usb::KCanUsbClass;

// ─── Shared channels ──────────────────────────────────────────────────────────

/// CAN RX → USB Bulk IN.
static CAN_TO_USB: Channel<CriticalSectionRawMutex, KCanFrame, 32> = Channel::new();

/// USB Bulk OUT → CAN TX.
static USB_TO_CAN: Channel<CriticalSectionRawMutex, KCanFrame, 32> = Channel::new();

// ─── Interrupt bindings ───────────────────────────────────────────────────────

bind_interrupts!(struct Irqs {
    OTG_FS => usb::InterruptHandler<peripherals::USB_OTG_FS>;
    FDCAN1_IT0 => can::IT0InterruptHandler<peripherals::FDCAN1>;
    FDCAN1_IT1 => can::IT1InterruptHandler<peripherals::FDCAN1>;
});

// ─── Entry point ─────────────────────────────────────────────────────────────

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // ── Clock configuration ───────────────────────────────────────────────────
    // System:  HSE 8 MHz → PLL1 → 480 MHz sysclk  (VoltageScale0)
    // FDCAN:   HSE 8 MHz → PLL2Q → 32 MHz
    // USB:     HSI48 with CRS (simplest, no PLL3 needed)
    let mut config = Config::default();
    {
        use embassy_stm32::rcc::*;
        config.rcc.hse = Some(Hse {
            freq: embassy_stm32::time::Hertz(8_000_000),
            mode: HseMode::Oscillator,
        });
        // PLL1: 8 MHz / 2 * 240 / 2 = 480 MHz sysclk.
        config.rcc.pll1 = Some(Pll {
            source: PllSource::HSE,
            prediv: PllPreDiv::DIV2,  // → 4 MHz
            mul: PllMul::MUL240,      // → 960 MHz VCO
            divp: Some(PllDiv::DIV2), // → 480 MHz sysclk
            divq: None,
            divr: None,
        });
        // PLL2: 8 MHz / 1 * 40 / 10 = 32 MHz FDCAN kernel clock.
        config.rcc.pll2 = Some(Pll {
            source: PllSource::HSE,
            prediv: PllPreDiv::DIV1, // → 8 MHz
            mul: PllMul::MUL40,      // → 320 MHz VCO
            divp: None,
            divq: Some(PllDiv::DIV10), // → 32 MHz
            divr: None,
        });
        config.rcc.sys = Sysclk::PLL1_P;
        config.rcc.ahb_pre = AHBPrescaler::DIV2; // 240 MHz HCLK
        config.rcc.apb1_pre = APBPrescaler::DIV2; // 120 MHz
        config.rcc.apb2_pre = APBPrescaler::DIV2; // 120 MHz
        config.rcc.apb3_pre = APBPrescaler::DIV2; // 120 MHz
        config.rcc.apb4_pre = APBPrescaler::DIV2; // 120 MHz
        config.rcc.voltage_scale = VoltageScale::Scale0;
        // FDCAN kernel clock from PLL2Q.
        config.rcc.mux.fdcansel = mux::Fdcansel::PLL2_Q;
        // USB clock from HSI48 (with CRS).
        config.rcc.hsi48 = Some(Hsi48Config {
            sync_from_usb: true,
        });
        config.rcc.mux.usbsel = mux::Usbsel::HSI48;
    }

    let p = embassy_stm32::init(config);

    info!("KCAN Dongle v1 — booting");

    // ── Status LEDs ───────────────────────────────────────────────────────────
    let led_bus_on = Output::new(p.PB0, Level::Low, Speed::Low);
    let led_rx = Output::new(p.PE1, Level::Low, Speed::Low);
    let led_err = Output::new(p.PB14, Level::Low, Speed::Low);

    // ── FDCAN1: configure and start at default 250 kbps ───────────────────────
    let mut can_cfg = can::CanConfigurator::new(p.FDCAN1, p.PD0, p.PD1, Irqs);
    can_cfg.set_bitrate(250_000);
    let can = can_cfg.into_normal_mode();

    // ── USB OTG FS ───────────────────────────────────────────────────────────
    // ep_out_buffer must be 'static for the driver lifetime.
    static mut USB_EP_OUT_BUF: [u8; 256] = [0u8; 256];
    let mut usb_cfg = embassy_stm32::usb::Config::default();
    usb_cfg.vbus_detection = false;
    // SAFETY: main is only ever called once by the embassy runtime.
    let usb_driver = Driver::new_fs(
        p.USB_OTG_FS,
        Irqs,
        p.PA12, // DP
        p.PA11, // DM
        unsafe { &mut *core::ptr::addr_of_mut!(USB_EP_OUT_BUF) },
        usb_cfg,
    );

    // ── USB device descriptor ─────────────────────────────────────────────────
    static mut CONFIG_DESCRIPTOR: [u8; 256] = [0u8; 256];
    static mut BOS_DESCRIPTOR: [u8; 256] = [0u8; 256];
    static mut CONTROL_BUF: [u8; 64] = [0u8; 64];

    let mut usb_config = embassy_usb::Config::new(0x1209, 0xBEEF);
    usb_config.manufacturer = Some("Kodezine");
    usb_config.product = Some("KCAN Dongle v1");
    usb_config.serial_number = Some("KCAN0001");
    usb_config.device_class = 0xFF;
    usb_config.device_sub_class = 0xFF;
    usb_config.device_protocol = 0xFF;

    // SAFETY: static mut buffers initialised once here before any tasks start.
    let mut builder = UsbBuilder::new(
        usb_driver,
        usb_config,
        unsafe { &mut *core::ptr::addr_of_mut!(CONFIG_DESCRIPTOR) },
        unsafe { &mut *core::ptr::addr_of_mut!(BOS_DESCRIPTOR) },
        &mut [],
        unsafe { &mut *core::ptr::addr_of_mut!(CONTROL_BUF) },
    );

    // ── KCAN USB class ───────────────────────────────────────────────────────
    let kcan_class = KCanUsbClass::new(&mut builder);
    let usb = builder.build();

    // ── Spawn tasks ───────────────────────────────────────────────────────────
    spawner.spawn(usb_task::usb_device_task(usb).ok().unwrap());
    spawner.spawn(
        usb_task::kcan_io_task(kcan_class, &CAN_TO_USB, &USB_TO_CAN)
            .ok()
            .unwrap(),
    );
    spawner.spawn(
        can_task::can_task(can, &CAN_TO_USB, &USB_TO_CAN)
            .ok()
            .unwrap(),
    );
    spawner.spawn(
        status_task::status_task(led_bus_on, led_rx, led_err, &CAN_TO_USB)
            .ok()
            .unwrap(),
    );

    info!("All tasks spawned — running");
}
