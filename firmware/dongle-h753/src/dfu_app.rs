//! DFU Runtime interface for the KCAN app firmware.
//!
//! Adds a USB DFU Runtime class (class 0xFE, subclass 0x01, protocol 0x01) to
//! the KCAN USB device descriptor.  When the host sends a `DFU_DETACH` request
//! (followed by a USB reset within the detach timeout), the [`AppDfuHandler`]
//! signals [`dfu_app_task`] which calls `mark_dfu()` and resets.
//!
//! On the next boot the bootloader sees `State::DfuDetach` and starts the USB
//! DFU download stack instead of jumping to the app.
//!
//! # mark_booted
//!
//! The first time USB is configured, `kcan_io_task` fires [`MARK_BOOTED`].
//! `dfu_app_task` responds by calling `mark_booted()` to suppress rollback.

use embassy_boot::{AlignedBuffer, BlockingFirmwareState};
use embassy_embedded_hal::flash::partition::BlockingPartition;
use embassy_stm32::flash::{Blocking, Flash};
use embassy_stm32::peripherals::FLASH;
use embassy_stm32::Peri;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use embassy_usb_dfu::application::{DfuAttributes, Handler};

use core::cell::RefCell;
use defmt::*;

pub use embassy_usb_dfu::application::DfuState;

// ── Signals ───────────────────────────────────────────────────────────────────

pub static ENTER_DFU: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub static MARK_BOOTED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

// ── Handler ───────────────────────────────────────────────────────────────────

pub struct AppDfuHandler;

impl Handler for AppDfuHandler {
    fn enter_dfu(&mut self) {
        info!("DFU: DFU_DETACH — signalling dfu_app_task");
        ENTER_DFU.signal(());
    }
}

pub fn make_dfu_state(handler: AppDfuHandler) -> DfuState<AppDfuHandler> {
    DfuState::new(
        handler,
        DfuAttributes::WILL_DETACH | DfuAttributes::MANIFESTATION_TOLERANT,
        Duration::from_millis(5000),
    )
}

// ── Async task ────────────────────────────────────────────────────────────────

const FLASH_WRITE_SIZE: usize = 32;

// Byte offsets within flash for the STATE partition.
// These match the __bootloader_state_{start,end} linker symbols emitted by
// build.rs.  Hard-coded here to avoid a dependency on the linker symbol
// extern declarations (which require unsafe and the correct linker script).
//
// STATE: 0x08100000 – 0x08120000, offset from flash start (0x08000000):
const STATE_OFFSET: u32 = 0x00100000;
const STATE_SIZE: u32 = 0x00020000; // 128 KB

#[embassy_executor::task]
pub async fn dfu_app_task(flash_peri: Peri<'static, FLASH>) {
    use embassy_futures::select::{select, Either};

    let flash = Flash::new_blocking(flash_peri);
    let flash_mutex: Mutex<NoopRawMutex, RefCell<Flash<'_, Blocking>>> =
        Mutex::new(RefCell::new(flash));

    loop {
        match select(MARK_BOOTED.wait(), ENTER_DFU.wait()).await {
            Either::First(()) => {
                info!("DFU app: marking firmware as booted");
                let state_part = BlockingPartition::new(&flash_mutex, STATE_OFFSET, STATE_SIZE);
                let mut aligned = AlignedBuffer([0u8; FLASH_WRITE_SIZE]);
                let mut state = BlockingFirmwareState::new(state_part, aligned.as_mut());
                match state.mark_booted() {
                    Ok(()) => info!("DFU app: mark_booted OK"),
                    Err(_) => warn!("DFU app: mark_booted failed"),
                }
            }
            Either::Second(()) => {
                info!("DFU app: writing DFU magic — rebooting");
                let state_part = BlockingPartition::new(&flash_mutex, STATE_OFFSET, STATE_SIZE);
                let mut aligned = AlignedBuffer([0u8; FLASH_WRITE_SIZE]);
                let mut state = BlockingFirmwareState::new(state_part, aligned.as_mut());
                match state.mark_dfu() {
                    Ok(()) => info!("DFU app: mark_dfu OK"),
                    Err(_) => warn!("DFU app: mark_dfu failed"),
                }
                Timer::after(Duration::from_millis(50)).await;
                cortex_m::peripheral::SCB::sys_reset();
            }
        }
    }
}
