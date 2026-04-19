//! Firmware download state machine — Rust port of `FtCop.c`.
//!
//! Implements the full CANopen bootloader flash programming sequence, matching
//! the C `BinaryBlockDownload` tool state machine exactly:
//!
//! CheckBootloader → (StartBootloader →) CheckVendorId → CheckProductCode
//!   → Clear → WaitClear → Download(loop) → FirstStartApp
//!   → DelayCheckApp → CheckAppWorks
//!     → (success) → done
//!     → (still bootloader) → RestartBootloader → SetSignature
//!       → WaitSetSignature → FinalStartApp → FinalCheckApp → done

use std::path::Path;
use std::thread;
use std::time::Duration;

use crate::file::BinaryBlockIter;
use crate::sdo_client::{SdoClient, SdoError, BLUPDATE_APP_DEVICE_TYPE, BOOTLOADER_DEVICE_TYPE};

// ─── Object indices ──────────────────────────────────────────────────────────

const OBJ_DEVICE_TYPE: u16 = 0x1000;
const OBJ_VENDOR_ID: u16 = 0x1018;
const SUB_VENDOR_ID: u8 = 0x01;
const OBJ_PRODUCT_CODE: u16 = 0x1018;
const SUB_PRODUCT_CODE: u8 = 0x02;
const OBJ_PROGRAM_DATA: u16 = 0x1F50;
const OBJ_PROGRAM_CONTROL: u16 = 0x1F51;
const OBJ_FLASH_STATUS: u16 = 0x1F57;

// ─── Program control commands (written to 0x1F51) ────────────────────────────

const CMD_START_APP: u32 = 0x01;
const CMD_CLEAR_PROGRAM: u32 = 0x03;
const CMD_START_BOOTLOADER: u32 = 0x80;
const CMD_SET_SIGNATURE: u32 = 0x83;

// ─── Flash status codes (read from 0x1F57) ───────────────────────────────────

const STAT_OK: u32 = 0x0000_0000;
const STAT_BUSY: u32 = 0x0000_0001;
const STAT_CRC_BUSY: u32 = 0x0000_0006;

// ─── Action type ─────────────────────────────────────────────────────────────

/// Which firmware download action to perform — mirrors `ActionType` from `FtCfg.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionType {
    /// Standard firmware update (default).
    UpdateFirmware,
    /// Load the bootloader-update application onto the device.
    LoadBlupdateApp,
    /// Update the bootloader itself (requires the blupdate-app already running).
    UpdateBootloader,
}

// ─── Configuration ───────────────────────────────────────────────────────────

/// Configuration for the firmware download state machine.
pub struct DownloadConfig {
    /// CANopen subindex of the program slot in 0x1F50/0x1F51/0x1F57 (1–127).
    pub program_number: u8,
    /// Vendor ID to validate (0 = skip check).
    pub vendor_id: u32,
    /// Product code to validate (0 = skip check).
    pub product_code: u32,
    /// Maximum retries while the bootloader reports BUSY.
    pub max_retries_busy: u32,
    /// Maximum retries while the bootloader reports CRC-BUSY.
    pub max_retries_crc: u32,
    /// Delay between flash-status polls (100 µs units).
    pub poll_delay_100us: u32,
    /// Delay before checking if the application started (100 µs units).
    pub delay_check_app_100us: u32,
    /// Delay before re-checking if the bootloader re-entered (100 µs units).
    pub delay_check_bl_100us: u32,
    /// Which action to perform.
    pub action: ActionType,
    /// Transfer mode for large block downloads.
    pub sdoc_type: crate::sdo_client::SdocType,
    /// Total steps (for progress display, passed through to future SysMsg integration).
    #[allow(dead_code)]
    pub total_steps: u8,
    /// Current step number (for progress display).
    #[allow(dead_code)]
    pub current_step: u8,
}

// ─── Progress callback ───────────────────────────────────────────────────────

/// Progress event emitted by the state machine.
#[derive(Debug, Clone)]
pub enum Progress {
    /// Entered a new state.
    State(&'static str),
    /// Block downloaded: `(block_num, bytes_transferred, total_bytes)`.
    BlockDownloaded {
        block_num: u32,
        bytes_done: u64,
        total_bytes: u64,
    },
    /// Overall percentage (0–100). Reserved for future progress-bar integration.
    #[allow(dead_code)]
    Percent(u8),
}

/// Errors returned by the state machine.
#[derive(Debug)]
pub enum DownloadError {
    /// An SDO transfer failed.
    Sdo(SdoError),
    /// The binary block file could not be read.
    File(crate::file::FileError),
    /// A hardware validation check failed (vendor ID, product code, etc.).
    Validation(String),
    /// The bootloader did not enter the expected state within the retry limit.
    BootloaderTimeout(String),
    /// The application failed to start after flashing.
    AppStartFailed,
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sdo(e) => write!(f, "SDO error: {e}"),
            Self::File(e) => write!(f, "file error: {e}"),
            Self::Validation(s) => write!(f, "validation error: {s}"),
            Self::BootloaderTimeout(s) => write!(f, "bootloader timeout: {s}"),
            Self::AppStartFailed => write!(f, "application failed to start after flashing"),
        }
    }
}

impl From<SdoError> for DownloadError {
    fn from(e: SdoError) -> Self {
        Self::Sdo(e)
    }
}

impl From<crate::file::FileError> for DownloadError {
    fn from(e: crate::file::FileError) -> Self {
        Self::File(e)
    }
}

// ─── Helper: duration from 100 µs units ─────────────────────────────────────

fn dur_100us(units: u32) -> Duration {
    Duration::from_micros(units as u64 * 100)
}

// ─── Helper: interpret flash status ─────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum FlashStatus {
    Ok,
    Busy,
    CrcBusy,
    Error(u32),
}

fn classify_flash_status(raw: u32) -> FlashStatus {
    match raw {
        STAT_OK => FlashStatus::Ok,
        STAT_BUSY => FlashStatus::Busy,
        STAT_CRC_BUSY => FlashStatus::CrcBusy,
        other => FlashStatus::Error(other),
    }
}

// ─── Public entry point ───────────────────────────────────────────────────────

/// Run the complete firmware download sequence.
///
/// `progress_cb` is called on significant events (state changes, block counts).
pub fn run_firmware_download(
    client: &mut SdoClient,
    cfg: &DownloadConfig,
    binary_path: &Path,
    progress_cb: &mut dyn FnMut(Progress),
) -> Result<(), DownloadError> {
    // ── 1. Check / enter bootloader ──────────────────────────────────────────
    progress_cb(Progress::State("CheckBootloader"));
    let bootloader_running = is_bootloader_active(client, cfg.action)?;
    if !bootloader_running {
        progress_cb(Progress::State("StartBootloader"));
        start_bootloader(client, cfg)?;
    }

    // ── 2. Validate vendor ID ────────────────────────────────────────────────
    if cfg.vendor_id != 0 {
        progress_cb(Progress::State("CheckVendorId"));
        let actual = client.read_u32(OBJ_VENDOR_ID, SUB_VENDOR_ID)?;
        if actual != cfg.vendor_id {
            return Err(DownloadError::Validation(format!(
                "vendor ID mismatch: expected 0x{:08X}, got 0x{:08X}",
                cfg.vendor_id, actual
            )));
        }
    }

    // ── 3. Validate product code ─────────────────────────────────────────────
    if cfg.product_code != 0 {
        progress_cb(Progress::State("CheckProductCode"));
        let actual = client.read_u32(OBJ_PRODUCT_CODE, SUB_PRODUCT_CODE)?;
        if actual != cfg.product_code {
            return Err(DownloadError::Validation(format!(
                "product code mismatch: expected 0x{:08X}, got 0x{:08X}",
                cfg.product_code, actual
            )));
        }
    }

    // ── 4. Clear flash ───────────────────────────────────────────────────────
    progress_cb(Progress::State("ClearFlash"));
    client.write_u32(OBJ_PROGRAM_CONTROL, cfg.program_number, CMD_CLEAR_PROGRAM)?;

    progress_cb(Progress::State("WaitClear"));
    wait_flash_status(client, cfg, cfg.max_retries_busy, cfg.max_retries_crc)?;

    // ── 5. Download blocks ───────────────────────────────────────────────────
    progress_cb(Progress::State("Download"));
    let iter = BinaryBlockIter::open(binary_path)?;
    let total_bytes = iter.total_size;

    for block_result in iter {
        let block = block_result?;
        let block_num = block.block_num;

        client.download(
            OBJ_PROGRAM_DATA,
            cfg.program_number,
            &block.raw,
            cfg.sdoc_type,
        )?;

        // Poll flash status after each block
        wait_flash_status(client, cfg, cfg.max_retries_busy, cfg.max_retries_crc)?;

        let bytes_done = {
            // Re-open not possible inside the loop; estimate from block.raw.len()
            // The total is tracked via BinaryBlockIter::bytes_read but we consume
            // the iterator, so we track manually.
            block.raw.len() as u64
        };
        progress_cb(Progress::BlockDownloaded {
            block_num,
            bytes_done,
            total_bytes,
        });
    }

    // ── 6. First start attempt ───────────────────────────────────────────────
    progress_cb(Progress::State("FirstStartApp"));
    client.write_u32(OBJ_PROGRAM_CONTROL, cfg.program_number, CMD_START_APP)?;

    progress_cb(Progress::State("DelayCheckApp"));
    thread::sleep(dur_100us(cfg.delay_check_app_100us));

    progress_cb(Progress::State("CheckAppWorks"));
    let app_running = !is_bootloader_active(client, cfg.action)?;

    if app_running {
        progress_cb(Progress::State("Done"));
        return Ok(());
    }

    // ── 7. App did not start — set signature path ────────────────────────────
    progress_cb(Progress::State("RestartBootloader"));
    client.write_u32(
        OBJ_PROGRAM_CONTROL,
        cfg.program_number,
        CMD_START_BOOTLOADER,
    )?;

    progress_cb(Progress::State("DelayCheckReenterBootloader"));
    thread::sleep(dur_100us(cfg.delay_check_bl_100us));

    progress_cb(Progress::State("CheckReenterBootloader"));
    let back_in_bl = is_bootloader_active(client, cfg.action)?;
    if !back_in_bl {
        return Err(DownloadError::BootloaderTimeout(
            "bootloader did not re-enter after first start attempt".into(),
        ));
    }

    progress_cb(Progress::State("SetSignature"));
    client.write_u32(OBJ_PROGRAM_CONTROL, cfg.program_number, CMD_SET_SIGNATURE)?;

    progress_cb(Progress::State("WaitSetSignature"));
    wait_flash_status(client, cfg, cfg.max_retries_busy, cfg.max_retries_crc)?;

    progress_cb(Progress::State("FinalStartApp"));
    client.write_u32(OBJ_PROGRAM_CONTROL, cfg.program_number, CMD_START_APP)?;

    progress_cb(Progress::State("DelayFinalCheckApp"));
    thread::sleep(dur_100us(cfg.delay_check_app_100us));

    progress_cb(Progress::State("FinalCheckApp"));
    let final_running = !is_bootloader_active(client, cfg.action)?;
    if !final_running {
        return Err(DownloadError::AppStartFailed);
    }

    progress_cb(Progress::State("Done"));
    Ok(())
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

/// Return `true` if the target device type indicates a bootloader (or blupdate-app).
fn is_bootloader_active(client: &mut SdoClient, action: ActionType) -> Result<bool, DownloadError> {
    let dev_type = client.read_u32(OBJ_DEVICE_TYPE, 0)?;
    Ok(match action {
        ActionType::UpdateFirmware | ActionType::LoadBlupdateApp => {
            dev_type == BOOTLOADER_DEVICE_TYPE
        }
        ActionType::UpdateBootloader => {
            dev_type == BLUPDATE_APP_DEVICE_TYPE || dev_type == BOOTLOADER_DEVICE_TYPE
        }
    })
}

/// Send `CMD_START_BOOTLOADER` and wait until the bootloader reports active.
fn start_bootloader(client: &mut SdoClient, cfg: &DownloadConfig) -> Result<(), DownloadError> {
    client.write_u32(
        OBJ_PROGRAM_CONTROL,
        cfg.program_number,
        CMD_START_BOOTLOADER,
    )?;

    thread::sleep(dur_100us(cfg.delay_check_bl_100us));

    // Retry up to max_retries_busy times waiting for the bootloader
    for attempt in 0..cfg.max_retries_busy {
        match is_bootloader_active(client, cfg.action) {
            Ok(true) => return Ok(()),
            Ok(false) => {
                if attempt + 1 < cfg.max_retries_busy {
                    thread::sleep(dur_100us(cfg.poll_delay_100us));
                }
            }
            Err(DownloadError::Sdo(SdoError::Timeout)) => {
                if attempt + 1 < cfg.max_retries_busy {
                    thread::sleep(dur_100us(cfg.poll_delay_100us));
                }
            }
            Err(e) => return Err(e),
        }
    }
    Err(DownloadError::BootloaderTimeout(
        "node did not enter bootloader within retry limit".into(),
    ))
}

/// Poll object 0x1F57 until the flash status is `OK`, retrying on `BUSY`/`CRC_BUSY`.
fn wait_flash_status(
    client: &mut SdoClient,
    cfg: &DownloadConfig,
    max_retries_busy: u32,
    max_retries_crc: u32,
) -> Result<(), DownloadError> {
    let mut busy_retries = 0u32;
    let mut crc_retries = 0u32;

    loop {
        thread::sleep(dur_100us(cfg.poll_delay_100us));

        let raw = client.read_u32(OBJ_FLASH_STATUS, cfg.program_number)?;
        match classify_flash_status(raw) {
            FlashStatus::Ok => return Ok(()),
            FlashStatus::Busy => {
                busy_retries += 1;
                if busy_retries >= max_retries_busy {
                    return Err(DownloadError::BootloaderTimeout(
                        "flash BUSY timeout exceeded".into(),
                    ));
                }
            }
            FlashStatus::CrcBusy => {
                crc_retries += 1;
                if crc_retries >= max_retries_crc {
                    return Err(DownloadError::BootloaderTimeout(
                        "flash CRC-BUSY timeout exceeded".into(),
                    ));
                }
            }
            FlashStatus::Error(code) => {
                return Err(DownloadError::Sdo(SdoError::Abort(code)));
            }
        }
    }
}
