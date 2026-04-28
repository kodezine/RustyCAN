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
//! | FDCAN1_RX     | PD0  | SN65HVD230 module 1 (channel 0)    |
//! | FDCAN1_TX     | PD1  | SN65HVD230 module 1 (channel 0)    |
//! | FDCAN2_RX     | PB5  | SN65HVD230 module 2 (channel 1)    |
//! | FDCAN2_TX     | PB6  | SN65HVD230 module 2 (channel 1)    |
//! | USB_OTG_FS_DM | PA11 | Micro-B connector CN13             |
//! | USB_OTG_FS_DP | PA12 | Micro-B connector CN13             |
//! | LD1 (green)   | PB0  | Heartbeat — 1 Hz blink             |
//! | LD2 (blue)    | PE1  | USB host connected (solid on)       |
//! | LD3 (red)     | PB14 | TX error blink (future)             |
//!
//! # Clocks
//!
//! System clock: 480 MHz (HSE + PLL1).
//! FDCAN kernel clock: 32 MHz (PLL2Q = 64 MHz / 2, configured below).
//! TIM2: 1 MHz free-running counter for hardware timestamps.
//! USB OTG FS: 48 MHz (HSI48 + CRS).

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

#[cfg(feature = "bus-test")]
mod bus_test;
mod can_task;
#[cfg(feature = "periodic-echo")]
mod echo_task;
mod ep0_handler;
mod kcan_usb;
mod status_task;
mod usb_task;

use kcan_protocol::frame::KCanFrame;
use kcan_usb::KCanUsbClass;

// ─── Shared channels ──────────────────────────────────────────────────────────

/// CAN RX → USB Bulk IN (shared by both channels; frames carry a channel field).
static CAN_TO_USB: Channel<CriticalSectionRawMutex, KCanFrame, 32> = Channel::new();

/// USB Bulk OUT → FDCAN1 TX (channel 0).
static USB_TO_CAN: Channel<CriticalSectionRawMutex, KCanFrame, 32> = Channel::new();

/// USB Bulk OUT → FDCAN2 TX (channel 1).
static USB_TO_CAN2: Channel<CriticalSectionRawMutex, KCanFrame, 32> = Channel::new();

/// RX-frame tap used by the cross-channel bus self-test (feature = "bus-test").
#[cfg(feature = "bus-test")]
static BUS_TEST_MONITOR: Channel<CriticalSectionRawMutex, KCanFrame, 8> = Channel::new();

// ─── Interrupt bindings ───────────────────────────────────────────────────────

bind_interrupts!(struct Irqs {
    OTG_FS     => usb::InterruptHandler<peripherals::USB_OTG_FS>;
    FDCAN1_IT0 => can::IT0InterruptHandler<peripherals::FDCAN1>;
    FDCAN1_IT1 => can::IT1InterruptHandler<peripherals::FDCAN1>;
    FDCAN2_IT0 => can::IT0InterruptHandler<peripherals::FDCAN2>;
    FDCAN2_IT1 => can::IT1InterruptHandler<peripherals::FDCAN2>;
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
        // CSI required by STM32H7 USB analog circuits (see embassy USB examples).
        config.rcc.csi = true;
        // FDCAN kernel clock from PLL2Q.
        config.rcc.mux.fdcansel = mux::Fdcansel::PLL2_Q;
        // USB clock from HSI48 (with CRS).
        config.rcc.hsi48 = Some(Hsi48Config {
            sync_from_usb: true,
        });
        config.rcc.mux.usbsel = mux::Usbsel::HSI48;
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

    info!("KCAN Dongle v1 — booting  uid={:08X}", uid_lo);
    info!("USB: VID=0x1209 PID=0xBEEF  serial={}", serial_str);

    // ── Status LEDs ───────────────────────────────────────────────────────────
    let led_bus_on = Output::new(p.PB0, Level::Low, Speed::Low);
    let led_rx = Output::new(p.PE1, Level::Low, Speed::Low);
    let led_err = Output::new(p.PB14, Level::Low, Speed::Low);

    // ── FDCAN1: channel 0 — SN65HVD230 module 1 (PD0/PD1) ───────────────────
    let mut can1_cfg = can::CanConfigurator::new(p.FDCAN1, p.PD0, p.PD1, Irqs);
    can1_cfg.set_bitrate(250_000);
    #[cfg(not(feature = "loopback"))]
    let can = {
        let c = can1_cfg.into_normal_mode();
        info!("FDCAN1: normal mode, 250 kbps (PLL2Q = 32 MHz)");
        c
    };
    #[cfg(feature = "loopback")]
    let can = {
        let c = can1_cfg.into_internal_loopback_mode();
        info!("FDCAN1: INTERNAL LOOPBACK mode, 250 kbps — Phase 2 self-test");
        c
    };

    // ── FDCAN2: channel 1 — SN65HVD230 module 2 (PB5/PB6) ────────────────────
    let mut can2_cfg = can::CanConfigurator::new(p.FDCAN2, p.PB5, p.PB6, Irqs);
    can2_cfg.set_bitrate(250_000);
    let can2 = {
        let c = can2_cfg.into_normal_mode();
        info!("FDCAN2: normal mode, 250 kbps (channel 1)");
        c
    };

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
        &mut [],
        unsafe { &mut *core::ptr::addr_of_mut!(CONTROL_BUF) },
    );

    // EP0 vendor handler: responds to GET_INFO / GET_BT_CONST and ACKs
    // SET_BITTIMING / SET_MODE; also drives USB_CONFIGURED signal.
    static mut EP0_HANDLER: ep0_handler::KCanEp0Handler = ep0_handler::KCanEp0Handler { uid_lo: 0 };
    // SAFETY: single-threaded init before tasks are spawned.
    let ep0 = unsafe { &mut *core::ptr::addr_of_mut!(EP0_HANDLER) };
    ep0.uid_lo = uid_lo;
    builder.handler(ep0);

    // ── KCAN USB class ───────────────────────────────────────────────────────
    let kcan_class = KCanUsbClass::new(&mut builder);
    let usb = builder.build();

    // ── Spawn tasks ───────────────────────────────────────────────────────────
    spawner.spawn(usb_task::usb_device_task(usb).ok().unwrap());
    spawner.spawn(
        usb_task::kcan_io_task(kcan_class, &CAN_TO_USB, &USB_TO_CAN, &USB_TO_CAN2)
            .ok()
            .unwrap(),
    );
    spawner.spawn(
        can_task::can_task(
            can,
            0,
            &CAN_TO_USB,
            &USB_TO_CAN,
            #[cfg(feature = "bus-test")]
            &BUS_TEST_MONITOR,
        )
        .ok()
        .unwrap(),
    );
    spawner.spawn(
        can_task::can_task(
            can2,
            1,
            &CAN_TO_USB,
            &USB_TO_CAN2,
            #[cfg(feature = "bus-test")]
            &BUS_TEST_MONITOR,
        )
        .ok()
        .unwrap(),
    );
    spawner.spawn(
        status_task::status_task(led_bus_on, led_rx, led_err, &CAN_TO_USB)
            .ok()
            .unwrap(),
    );
    #[cfg(feature = "bus-test")]
    spawner.spawn(
        bus_test::bus_test_task(&USB_TO_CAN, &USB_TO_CAN2, &BUS_TEST_MONITOR)
            .ok()
            .unwrap(),
    );
    #[cfg(feature = "periodic-echo")]
    spawner.spawn(
        echo_task::echo_task(&USB_TO_CAN, &USB_TO_CAN2)
            .ok()
            .unwrap(),
    );

    info!("All tasks spawned — ready for USB enumeration");
}
