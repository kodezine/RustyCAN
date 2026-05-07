//! Bootloader size probe — measures the flash footprint of the real bootloader
//! dependencies before committing to the 128 KB partition boundary.
//!
//! Build and measure:
//! ```sh
//! cd firmware
//! cargo build --release -p bootloader-size-probe
//! cargo size --release -p bootloader-size-probe
//! ```
//!
//! Decision rule (measured .text + .rodata):
//!   ≤ 96 KB  → 128 KB partition confirmed (app start 0x08020000)
//!   97–160 KB → widen to 192 KB           (app start 0x08030000)
//!   161–224 KB → widen to 256 KB          (app start 0x08040000)
//!
//! DELETE this crate once the partition addresses are finalised.

#![no_std]
#![no_main]

use embassy_boot::{AlignedBuffer, BlockingFirmwareUpdater, FirmwareUpdaterConfig};
use embassy_boot_stm32::BootLoaderConfig;
use embassy_executor::Spawner;
use embassy_stm32::flash::{Blocking, Flash};
use embassy_stm32::usb::Driver;
use embassy_stm32::{bind_interrupts, peripherals, usb, Config};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_usb::Builder as UsbBuilder;
use embassy_usb_dfu::consts::DfuAttributes;
use embassy_usb_dfu::dfu::new_state;
use embassy_usb_dfu::ResetImmediate;

use core::cell::RefCell;
use defmt::*;
use defmt_rtt as _;
use panic_probe as _;

bind_interrupts!(struct Irqs {
    OTG_HS => usb::InterruptHandler<peripherals::USB_OTG_HS>;
});

// Flash write size for STM32H7 (32 bytes = minimum write granularity).
const FLASH_WRITE_SIZE: usize = 32;

// Signing public key (32 bytes) — placeholder zeros for the size probe.
static PUBLIC_KEY: [u8; 32] = [0u8; 32];

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_stm32::init(Config::default());

    // ── Flash partitions via embassy-boot linker symbols ──────────────────────
    let flash = Flash::new_blocking(p.FLASH);
    // Wrap in Mutex<NoopRawMutex, RefCell<_>> as required by
    // BootLoaderConfig::from_linkerfile_blocking.
    let flash_mutex: Mutex<NoopRawMutex, RefCell<Flash<'_, Blocking>>> =
        Mutex::new(RefCell::new(flash));

    let config =
        BootLoaderConfig::from_linkerfile_blocking(&flash_mutex, &flash_mutex, &flash_mutex);

    // FirmwareUpdaterConfig for the DFU+STATE partitions.
    let updater_config =
        embassy_boot::FirmwareUpdaterConfig::from_linkerfile_blocking(&flash_mutex, &flash_mutex);
    let mut aligned = embassy_boot::AlignedBuffer([0u8; FLASH_WRITE_SIZE]);
    let updater = BlockingFirmwareUpdater::new(updater_config, aligned.as_mut());

    // ── USB OTG HS (ULPI) ─────────────────────────────────────────────────────
    static mut USB_EP_OUT_BUF: [u8; 1024] = [0u8; 1024];
    let mut usb_cfg = embassy_stm32::usb::Config::default();
    usb_cfg.vbus_detection = false;
    // SAFETY: main() called once.
    let driver = Driver::new_hs_ulpi(
        p.USB_OTG_HS,
        Irqs,
        p.PA5,  // ULPI_CLK
        p.PI11, // ULPI_DIR
        p.PH4,  // ULPI_NXT
        p.PC0,  // ULPI_STP
        p.PA3,
        p.PB0,
        p.PB1,
        p.PB10,
        p.PB11,
        p.PB12,
        p.PB13,
        p.PB5,
        unsafe { &mut *core::ptr::addr_of_mut!(USB_EP_OUT_BUF) },
        usb_cfg,
    );

    // ── DFU Class — bootloader download mode (with ed25519 verification) ─────
    // Declared before builder so it outlives the builder's borrow.
    let mut dfu_state = new_state::<_, _, _, FLASH_WRITE_SIZE>(
        updater,
        DfuAttributes::CAN_DOWNLOAD | DfuAttributes::MANIFESTATION_TOLERANT,
        ResetImmediate,
        &PUBLIC_KEY,
    );

    let mut usb_config = embassy_usb::Config::new(0x1209, 0xBEEF);
    usb_config.manufacturer = Some("Kodezine");
    usb_config.product = Some("KCAN DFU H743");
    usb_config.serial_number = Some("00000000");
    usb_config.max_power = 100;
    usb_config.max_packet_size_0 = 64;

    static mut CONFIG_DESC: [u8; 256] = [0u8; 256];
    static mut BOS_DESC: [u8; 256] = [0u8; 256];
    static mut MSOS_DESC: [u8; 256] = [0u8; 256];
    static mut CTRL_BUF: [u8; 64] = [0u8; 64];

    // SAFETY: static mut buffers, initialised once in main.
    let mut builder = unsafe {
        UsbBuilder::new(
            driver,
            usb_config,
            &mut CONFIG_DESC,
            &mut BOS_DESC,
            &mut MSOS_DESC,
            &mut CTRL_BUF,
        )
    };

    embassy_usb_dfu::dfu::usb_dfu(&mut builder, &mut dfu_state, |_| {});

    let mut usb = builder.build();
    usb.run().await;
}
