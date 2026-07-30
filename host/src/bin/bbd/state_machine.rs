//! Firmware download state machine — Rust port of `FtCop.c`.
//!
//! Implements the full CANopen bootloader flash programming sequence, matching
//! the C `BinaryBlockDownload` tool state machine exactly:
//!
//! CheckBootloader → (StartBootloader →) CheckVendorId → CheckProductCode
//!   → Clear → WaitClear → Download(loop) → FirstStartApp
//!   → DelayCheckApp → CheckAppWorks
//!     → (still bootloader) → error: image broken, no signature written
//!     → (app running)  → RestartBootloader → CheckReenterBootloader
//!       → SetSignature → WaitSetSignature → FinalStartApp → FinalCheckApp → done

use std::path::Path;
use std::thread;
use std::time::Duration;

use crate::file::BinaryBlockIter;
use crate::sdo_client::{SdoClient, SdoError, BLUPDATE_APP_DEVICE_TYPE, BOOTLOADER_DEVICE_TYPE};
use rustycan::adapters::AdapterError;

// ─── Object indices ──────────────────────────────────────────────────────────

const OBJ_DEVICE_TYPE: u16 = 0x1000;
const OBJ_VENDOR_ID: u16 = 0x1018;
const SUB_VENDOR_ID: u8 = 0x01;
const OBJ_PRODUCT_CODE: u16 = 0x1018;
const SUB_PRODUCT_CODE: u8 = 0x02;
/// Bootloader version: 0x1018 subindex 5, UNSIGNED32, format 0xXXYYZZDD.
const OBJ_BL_VERSION: u16 = 0x1018;
const SUB_BL_VERSION: u8 = 0x05;
/// Application software version: 0x100A, visible string, app only.
const OBJ_APP_VERSION: u16 = 0x100A;
const OBJ_PROGRAM_DATA: u16 = 0x1F50;
const OBJ_PROGRAM_CONTROL: u16 = 0x1F51;
const OBJ_FLASH_STATUS: u16 = 0x1F57;

// ─── Program control commands (written to 0x1F51) ────────────────────────────

const CMD_START_APP: u8 = 0x01;
const CMD_CLEAR_PROGRAM: u8 = 0x03;
const CMD_START_BOOTLOADER: u8 = 0x80;
const CMD_SET_SIGNATURE: u8 = 0x83;

// ─── SDO abort codes ─────────────────────────────────────────────────────────

/// "Client/server command specifier not valid" — the abort libPCBUSB spuriously
/// returns on the first (cold) SDO exchange after the channel opens. Only this
/// code is treated as a transient worth retrying; every other abort is real.
const SDO_ABORT_COMMAND_INVALID: u32 = 0x0504_0001;

// ─── Flash status codes (read from 0x1F57) ───────────────────────────────────

const STAT_OK: u32 = 0x0000_0000;
const STAT_BUSY: u32 = 0x0000_0001;
const STAT_CRC_BUSY: u32 = 0x0000_0006;
/// Device-specific "erase in progress" status returned by some bootloaders.
const STAT_ERASE_BUSY: u32 = 0x1000_0000;

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

/// Summary of a completed firmware download.
#[derive(Debug, Clone, Default)]
pub struct DownloadSummary {
    /// Application version string (0x100A) read before entering the bootloader.
    /// `None` if the bootloader was already active at startup (old version unknown).
    pub app_version_before: Option<String>,
    /// Bootloader version from 0x1018/5 (0xXXYYZZDD), read while bootloader is active.
    pub bootloader_version: Option<u32>,
    /// Application version string (0x100A) read after the new application started.
    pub app_version_after: Option<String>,
    /// Total number of blocks written.
    pub blocks_written: u32,
}

/// Progress event emitted by the state machine.
#[derive(Debug, Clone)]
pub enum Progress {
    /// Entered a new state.
    State(&'static str),
    /// CheckBootloader completed; indicates what was running at startup.
    CheckBootloader {
        /// `true` if the bootloader was already active at startup.
        already_in_bootloader: bool,
    },
    /// Block downloaded: `(bytes_transferred, total_bytes)`.
    BlockDownloaded { bytes_done: u64, total_bytes: u64 },
    /// Application confirmed running after a start command. Carries the raw
    /// device-type value read from object 0x1000 (any non-bootloader value).
    AppWorking { device_type: u32 },
    /// Bootloader confirmed active again after returning from the application.
    /// Carries the raw device-type value read from object 0x1000.
    BootloaderReentered { device_type: u32 },
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
    /// The bootloader reported a non-OK flash status on object 0x1F57.
    /// Carries the raw status DWORD; decoded to a name by `flash_status_name`.
    FlashStatus(u32),
}

/// Decode a non-busy 0x1F57 flash-status DWORD into a human-readable reason.
///
/// Status layout (bootloader `BLCOP_STAT_WRITE`): `busy ? 1 : (err << 1) | (manu << 16)`.
/// So the error code is `status >> 1`; these names mirror the bootloader's
/// `BLCOP_STATERR_*` definitions.
fn flash_status_name(raw: u32) -> &'static str {
    match (raw >> 1) & 0x7FFF {
        0x00 => "OK",
        0x01 => "NOVALPROG (no valid program — application CRC check failed)",
        0x02 => "FORMAT (invalid block format)",
        0x03 => "CRC (block CRC error)",
        0x04 => "NOTCLEARED (flash not cleared before write)",
        0x05 => "WRITE (flash write error)",
        0x06 => "ADDRESS (invalid flash address)",
        0x07 => "SECURED (flash is read/write protected)",
        0x08 => "NVDATA (non-volatile data error)",
        0x3F => "UNSPECIFIED",
        _ => "unknown flash error",
    }
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sdo(e) => write!(f, "SDO error: {e}"),
            Self::File(e) => write!(f, "file error: {e}"),
            Self::Validation(s) => write!(f, "validation error: {s}"),
            Self::BootloaderTimeout(s) => write!(f, "bootloader timeout: {s}"),
            Self::AppStartFailed => write!(f, "application failed to start after flashing"),
            Self::FlashStatus(raw) => write!(
                f,
                "bootloader flash status 0x{raw:08X}: {}",
                flash_status_name(*raw)
            ),
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
        STAT_BUSY | STAT_ERASE_BUSY => FlashStatus::Busy,
        STAT_CRC_BUSY => FlashStatus::CrcBusy,
        other => FlashStatus::Error(other),
    }
}

// ─── Public entry point ───────────────────────────────────────────────────────

/// Run the complete firmware download sequence.
///
/// `progress_cb` is called on significant events (state changes, block counts).
/// Returns a [`DownloadSummary`] with version information on success.
pub fn run_firmware_download(
    client: &mut SdoClient,
    cfg: &DownloadConfig,
    binary_path: &Path,
    progress_cb: &mut dyn FnMut(Progress),
) -> Result<DownloadSummary, DownloadError> {
    let mut summary = DownloadSummary::default();

    // ── 0. Reset node SDO state ───────────────────────────────────────────────
    // If a previous run was interrupted mid-transfer the node's SDO server may
    // still be expecting download segments. Send an abort to clear that state
    // before issuing any new requests. No response is expected — ignore errors.
    let _ = client.send_abort(OBJ_PROGRAM_DATA, cfg.program_number);

    // ── 1. Check / enter bootloader ──────────────────────────────────────────
    let bootloader_running = is_bootloader_active(client, cfg.action)?;
    progress_cb(Progress::CheckBootloader {
        already_in_bootloader: bootloader_running,
    });
    if !bootloader_running {
        // App is running — read its version before switching to bootloader.
        summary.app_version_before = client.read_string(OBJ_APP_VERSION, 0).ok();
        progress_cb(Progress::State("StartBootloader"));
        start_bootloader(client, cfg)?;
    }

    // Bootloader is now active — read its version.
    summary.bootloader_version = client.read_u32(OBJ_BL_VERSION, SUB_BL_VERSION).ok();

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
    client.write_u8(OBJ_PROGRAM_CONTROL, cfg.program_number, CMD_CLEAR_PROGRAM)?;

    progress_cb(Progress::State("WaitClear"));
    wait_flash_status(client, cfg, cfg.max_retries_busy, cfg.max_retries_crc)?;

    // ── 5. Download blocks ───────────────────────────────────────────────────
    progress_cb(Progress::State("Download"));
    let iter = BinaryBlockIter::open(binary_path)?;
    let total_bytes = iter.total_size;
    let mut bytes_so_far: u64 = 0;

    for block_result in iter {
        let block = block_result?;

        client.download(
            OBJ_PROGRAM_DATA,
            cfg.program_number,
            &block.raw,
            cfg.sdoc_type,
        )?;

        // Poll flash status after each block
        wait_flash_status(client, cfg, cfg.max_retries_busy, cfg.max_retries_crc)?;

        bytes_so_far += block.raw.len() as u64;
        summary.blocks_written += 1;
        progress_cb(Progress::BlockDownloaded {
            bytes_done: bytes_so_far,
            total_bytes,
        });
    }

    // ── 6. First start — smoke test ──────────────────────────────────────────
    progress_cb(Progress::State("FirstStartApp"));
    write_control_expect_reset(client, cfg, CMD_START_APP)?;

    progress_cb(Progress::State("DelayCheckApp"));
    thread::sleep(dur_100us(cfg.delay_check_app_100us));

    progress_cb(Progress::State("CheckAppWorks"));
    // Poll 0x1000 until the application answers with any non-bootloader device
    // type. This tolerates the transient SDO timeout / adapter disconnect that
    // occurs while the node hands control from the bootloader to the app, and
    // works for every application on this bootloader (not just one).
    let app_dev_type = wait_app_running(client, cfg)?;
    progress_cb(Progress::AppWorking {
        device_type: app_dev_type,
    });

    // App verified — read its version while it's running.
    summary.app_version_after = client.read_string(OBJ_APP_VERSION, 0).ok();
    if summary.bootloader_version.is_none() {
        summary.bootloader_version = client.read_u32(OBJ_BL_VERSION, SUB_BL_VERSION).ok();
    }

    // ── 7. Return to bootloader to write signature ───────────────────────────
    // Only the bootloader has write access to the protected flash region where
    // the CRC signature lives. The application cannot write flash, so we must
    // switch back to bootloader mode before issuing CMD_SET_SIGNATURE.
    progress_cb(Progress::State("RestartBootloader"));
    write_control_expect_reset(client, cfg, CMD_START_BOOTLOADER)?;

    progress_cb(Progress::State("DelayCheckReenterBootloader"));
    thread::sleep(dur_100us(cfg.delay_check_bl_100us));

    progress_cb(Progress::State("CheckReenterBootloader"));
    // Poll 0x1000 until the bootloader answers again, riding through the SDO
    // timeout / adapter disconnect that the app→bootloader reset produces.
    let bl_dev_type = wait_bootloader_active(client, cfg)?;
    progress_cb(Progress::BootloaderReentered {
        device_type: bl_dev_type,
    });

    progress_cb(Progress::State("SetSignature"));
    client.write_u8(OBJ_PROGRAM_CONTROL, cfg.program_number, CMD_SET_SIGNATURE)?;

    progress_cb(Progress::State("WaitSetSignature"));
    wait_flash_status(client, cfg, cfg.max_retries_busy, cfg.max_retries_crc)?;

    // ── 8. Final start — app is now permanent ────────────────────────────────
    progress_cb(Progress::State("FinalStartApp"));
    write_control_expect_reset(client, cfg, CMD_START_APP)?;

    progress_cb(Progress::State("DelayFinalCheckApp"));
    thread::sleep(dur_100us(cfg.delay_check_app_100us));

    progress_cb(Progress::State("FinalCheckApp"));
    let final_dev_type = wait_app_running(client, cfg)?;
    progress_cb(Progress::AppWorking {
        device_type: final_dev_type,
    });

    progress_cb(Progress::State("Done"));
    Ok(summary)
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

/// Return `true` if the target device type indicates a bootloader (or blupdate-app).
fn is_bootloader_active(client: &mut SdoClient, action: ActionType) -> Result<bool, DownloadError> {
    let dev_type = client.read_u32(OBJ_DEVICE_TYPE, 0)?;
    Ok(device_type_is_bootloader(dev_type, action))
}

/// Classify a raw object-0x1000 device-type value as bootloader (or blupdate-app)
/// versus a running application, without performing any SDO I/O.
fn device_type_is_bootloader(dev_type: u32, action: ActionType) -> bool {
    match action {
        ActionType::UpdateFirmware | ActionType::LoadBlupdateApp => {
            dev_type == BOOTLOADER_DEVICE_TYPE
        }
        ActionType::UpdateBootloader => {
            dev_type == BLUPDATE_APP_DEVICE_TYPE || dev_type == BOOTLOADER_DEVICE_TYPE
        }
    }
}

/// Maximum wall-clock time to wait for a node to start responding on object
/// 0x1000 after a program-control command. Used both when waiting for the
/// application to come up and when waiting for the bootloader to re-enter
/// (both poll 0x1000).
const APP_START_TIMEOUT: Duration = Duration::from_secs(30);

/// Issue a program-control command (0x1F51) that is expected to trigger a mode
/// change on the node — starting the application or returning to the bootloader.
///
/// The node frequently resets and re-initialises its CAN controller *before* it
/// acknowledges the SDO write, so the write can come back as an SDO timeout or a
/// transient adapter error (a bus-off/error burst that the Summit backend reports
/// as `Disconnected`). Those are the *expected* result of a successful mode
/// switch, not a failure — the caller confirms the new mode by polling 0x1000.
///
/// The very first SDO exchange after the channel opens is also unreliable on
/// libPCBUSB: the cold frame is occasionally lost or answered with the spurious
/// [`SDO_ABORT_COMMAND_INVALID`] (0x05040001) abort, which is why a fresh run so
/// often had to be issued twice. The control write is idempotent and `write_u8`
/// drains stale RX before each attempt, so retry a few times on *that specific*
/// abort before giving up. Every other abort code (access denied, object not
/// found, …) is a genuine rejection and is surfaced immediately.
fn write_control_expect_reset(
    client: &mut SdoClient,
    cfg: &DownloadConfig,
    command: u8,
) -> Result<(), DownloadError> {
    const MAX_ATTEMPTS: u32 = 3;
    let mut last_abort = None;
    for _ in 0..MAX_ATTEMPTS {
        match client.write_u8(OBJ_PROGRAM_CONTROL, cfg.program_number, command) {
            Ok(()) => return Ok(()),
            // The node resets and re-initialises its CAN controller before it
            // ACKs, so a successful mode switch surfaces as an SDO timeout or the
            // transient USB/bus disconnect the Summit backend reports as
            // `Disconnected`. The caller confirms the new mode by polling 0x1000.
            Err(SdoError::Timeout) | Err(SdoError::Adapter(AdapterError::Disconnected)) => {
                return Ok(())
            }
            // Only the known cold-start abort is transient — re-issue it after
            // letting the bus settle. All other aborts are genuine and fall
            // through to the arm below.
            Err(SdoError::Abort(code)) if code == SDO_ABORT_COMMAND_INVALID => {
                last_abort = Some(SdoError::Abort(code));
                thread::sleep(Duration::from_millis(50));
            }
            // Any other error (a real SDO abort, Io/Protocol/Fatal/NotFound) is
            // a genuine failure and is surfaced immediately.
            Err(e) => return Err(DownloadError::Sdo(e)),
        }
    }
    Err(DownloadError::Sdo(last_abort.unwrap_or(SdoError::Timeout)))
}

/// Poll object 0x1000 until the bootloader (or blupdate-app) answers again — i.e.
/// the node has returned from the application to the bootloader — and return the
/// raw value. Tolerates the transient SDO timeout / adapter disconnect produced
/// by the application→bootloader reset, retrying until the deadline.
fn wait_bootloader_active(
    client: &mut SdoClient,
    cfg: &DownloadConfig,
) -> Result<u32, DownloadError> {
    let deadline = std::time::Instant::now() + APP_START_TIMEOUT;
    loop {
        match client.read_u32(OBJ_DEVICE_TYPE, 0) {
            Ok(dev_type) => {
                if device_type_is_bootloader(dev_type, cfg.action) {
                    return Ok(dev_type);
                }
                // Application still running — the reset has not completed yet.
            }
            // Expected transients while the node resets and re-initialises its
            // CAN controller: keep polling until the deadline.
            Err(SdoError::Timeout) | Err(SdoError::Adapter(AdapterError::Disconnected)) => {}
            // A real SDO abort / protocol / hard adapter error is not part of the
            // reset handshake — surface it immediately instead of masking it
            // behind the generic bootloader-timeout after the full window.
            Err(e) => return Err(DownloadError::Sdo(e)),
        }
        if std::time::Instant::now() >= deadline {
            return Err(DownloadError::BootloaderTimeout(
                "node did not enter the bootloader within the timeout window".into(),
            ));
        }
        thread::sleep(dur_100us(cfg.poll_delay_100us));
    }
}

/// Poll object 0x1000 until it reports a non-bootloader device type — i.e. the
/// application is running — and return that raw value.
///
/// The bootloader→application handover briefly makes the node unreachable (the
/// app re-initialises the CAN controller, which can surface as an SDO timeout or
/// even a momentary adapter disconnect). Those transient errors are treated as
/// "not up yet, keep polling" rather than a hard failure. Any value other than
/// the bootloader device type counts as the application working, so this works
/// for every application built on this bootloader — not just one.
fn wait_app_running(client: &mut SdoClient, cfg: &DownloadConfig) -> Result<u32, DownloadError> {
    let deadline = std::time::Instant::now() + APP_START_TIMEOUT;
    loop {
        match client.read_u32(OBJ_DEVICE_TYPE, 0) {
            Ok(dev_type) => {
                if !device_type_is_bootloader(dev_type, cfg.action) {
                    return Ok(dev_type);
                }
                // Still the bootloader — the app has not taken over yet.
            }
            // Expected transients during the bootloader→application handover:
            // keep polling until the deadline.
            Err(SdoError::Timeout) | Err(SdoError::Adapter(AdapterError::Disconnected)) => {}
            // Any other error (SDO abort, protocol, hard adapter failure) is a
            // genuine problem — propagate it immediately rather than hiding it
            // behind the generic app-start timeout.
            Err(e) => return Err(DownloadError::Sdo(e)),
        }
        if std::time::Instant::now() >= deadline {
            return Err(DownloadError::AppStartFailed);
        }
        thread::sleep(dur_100us(cfg.poll_delay_100us));
    }
}

/// Send `CMD_START_BOOTLOADER` and wait until the bootloader reports active.
fn start_bootloader(client: &mut SdoClient, cfg: &DownloadConfig) -> Result<(), DownloadError> {
    // Commanding the running application back into the bootloader resets the
    // node before it can ACK the 0x1F51 write, so the write surfaces as an SDO
    // timeout / adapter disconnect — the same expected transient the
    // post-download RestartBootloader step already tolerates. Treat it as a
    // successful mode switch and confirm by polling 0x1000, instead of failing
    // hard on the missing ACK.
    write_control_expect_reset(client, cfg, CMD_START_BOOTLOADER)?;

    thread::sleep(dur_100us(cfg.delay_check_bl_100us));

    wait_bootloader_active(client, cfg)?;
    Ok(())
}

/// Maximum wall-clock time to wait for flash erase / CRC to complete.
const FLASH_WAIT_TIMEOUT: Duration = Duration::from_secs(120);

/// Poll object 0x1F57 until the flash status is `OK`, with a 120-second
/// wall-clock deadline.
///
/// Some bootloaders reject SDO requests entirely while flash erase is in
/// progress, responding with an SDO abort or timeout. Those are treated as
/// BUSY and retried until the deadline expires.
fn wait_flash_status(
    client: &mut SdoClient,
    cfg: &DownloadConfig,
    _max_retries_busy: u32,
    _max_retries_crc: u32,
) -> Result<(), DownloadError> {
    let deadline = std::time::Instant::now() + FLASH_WAIT_TIMEOUT;

    loop {
        thread::sleep(dur_100us(cfg.poll_delay_100us));

        if std::time::Instant::now() >= deadline {
            return Err(DownloadError::BootloaderTimeout(
                "flash operation timed out after 120 s".into(),
            ));
        }

        let raw = match client.read_u32(OBJ_FLASH_STATUS, cfg.program_number) {
            Ok(v) => v,
            Err(SdoError::Abort(_)) | Err(SdoError::Timeout) | Err(SdoError::Protocol(_)) => {
                // Bootloader is busy (e.g. erasing) and cannot service SDO
                // requests — it may return a timeout, abort, or an unexpected
                // response frame. Keep waiting until the deadline.
                continue;
            }
            Err(e) => return Err(DownloadError::Sdo(e)),
        };

        match classify_flash_status(raw) {
            FlashStatus::Ok => return Ok(()),
            FlashStatus::Busy | FlashStatus::CrcBusy => {
                // Still erasing/verifying — keep polling.
            }
            FlashStatus::Error(code) => {
                return Err(DownloadError::FlashStatus(code));
            }
        }
    }
}
