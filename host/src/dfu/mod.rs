//! USB DFU Class 1.1 firmware update for the KCAN dongle.
//!
//! # Flow
//!
//! ```text
//! KCanAdapter::enter_dfu_mode()   ← DFU_DETACH via DFU Runtime interface
//!        ↓  (device resets into bootloader)
//! wait_for_dfu_device()           ← poll until VID:PID appears in DFU mode
//!        ↓
//! flash_firmware(path, pubkey)    ← verify Ed25519, then DFU download
//! ```
//!
//! # DFU Class 1.1 transfer sequence
//!
//! For each 64-byte block:
//!   1. DFU_DNLOAD (bmRequestType=0x21, bRequest=1, wValue=block_num, data)
//!   2. DFU_GETSTATUS (6 bytes) — wait dfuDNBUSY→dfuDNLOAD_IDLE
//!   3. (repeat)
//!
//! After last block (zero-length DNLOAD):
//!
//!   4. DFU_DNLOAD (wValue=N, data=[]) — signals end-of-transfer
//!   5. DFU_GETSTATUS — wait dfuMANIFEST_SYNC→dfuMANIFEST
//!   6. DFU_GETSTATUS — device reboots (MANIFESTATION_TOLERANT)

use std::path::Path;
use std::thread;
use std::time::Duration;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use nusb::DeviceInfo;
use nusb::MaybeFuture;
use rusb::{Context, DeviceHandle, UsbContext};
use sha2::{Digest, Sha512};

use kcan_protocol::control::{KCanDeviceInfo, RequestCode};

/// Called periodically during DFU_DNLOAD: `(blocks_done, total_blocks)`.
pub type ProgressCallback = Box<dyn Fn(usize, usize) + Send>;

const KCAN_VID: u16 = 0x1209;
const KCAN_PID: u16 = 0xBEEF;

// DFU Class 1.1 request codes
const DFU_DNLOAD: u8 = 1;
const DFU_GETSTATUS: u8 = 3;
const DFU_CLRSTATUS: u8 = 4;
const DFU_ABORT: u8 = 6;

// DFU states (bState in GETSTATUS response)
#[allow(dead_code)]
const APP_IDLE: u8 = 0;
const DFU_IDLE: u8 = 2;
const DFU_DNLOAD_SYNC: u8 = 3;
const DFU_DNBUSY: u8 = 4;
const DFU_DNLOAD_IDLE: u8 = 5;
const DFU_MANIFEST_SYNC: u8 = 6;
const DFU_MANIFEST: u8 = 7;
const DFU_ERROR: u8 = 10;

// DFU status codes (bStatus)
const STATUS_OK: u8 = 0x00;

const DFU_BLOCK_SIZE: usize = 64;

/// Signature file format: raw binary followed by 64-byte Ed25519 signature.
/// Matches the output of `sign-firmware --key` and the format expected by
/// `embassy-usb-dfu` on the device (which reads the last 64 bytes as signature).
const SIG_SUFFIX_LEN: usize = 64;

#[derive(Debug)]
pub enum DfuError {
    /// No KCAN device found in DFU mode after polling
    DeviceNotFound,
    /// Could not open or claim the USB device
    UsbOpen(String),
    /// DFU protocol error (bad status/state)
    Protocol(String),
    /// Signature verification failed
    SignatureInvalid,
    /// Binary file I/O error
    Io(std::io::Error),
}

impl std::fmt::Display for DfuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DfuError::DeviceNotFound => write!(f, "KCAN device not found in DFU mode"),
            DfuError::UsbOpen(e) => write!(f, "USB open error: {e}"),
            DfuError::Protocol(e) => write!(f, "DFU protocol error: {e}"),
            DfuError::SignatureInvalid => write!(f, "firmware signature verification failed"),
            DfuError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for DfuError {}

impl From<std::io::Error> for DfuError {
    fn from(e: std::io::Error) -> Self {
        DfuError::Io(e)
    }
}

/// Wait up to `timeout` for the KCAN device to appear in DFU mode.
///
/// The bootloader enumerates with the same VID:PID but bDeviceClass=0xFE/01/02.
pub fn wait_for_dfu_device(timeout: Duration) -> Result<DeviceInfo, DfuError> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(info) = find_dfu_device() {
            return Ok(info);
        }
        if std::time::Instant::now() >= deadline {
            return Err(DfuError::DeviceNotFound);
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn find_dfu_device() -> Option<DeviceInfo> {
    nusb::list_devices().wait().ok()?.find(|d: &DeviceInfo| {
        d.vendor_id() == KCAN_VID
            && d.product_id() == KCAN_PID
            && d.class() == 0xFE
            && d.subclass() == 0x01
            && d.protocol() == 0x02
    })
}

/// Query the running KCAN app's firmware version via EP0 GET_INFO.
///
/// Returns `(major, minor, patch)` or an error if the device is not found or
/// the request fails.  The device must be in app mode (not DFU bootloader).
pub fn get_device_firmware_version(serial: Option<&str>) -> Result<(u8, u8, u8), DfuError> {
    use nusb::transfer::{ControlIn, ControlType, Recipient};

    let dev = nusb::list_devices()
        .wait()
        .map_err(|e| DfuError::UsbOpen(format!("list devices: {e}")))?
        .find(|d: &DeviceInfo| {
            d.vendor_id() == KCAN_VID
                && d.product_id() == KCAN_PID
                // App mode: class 0x00 (per-interface) or 0xFF, NOT 0xFE (DFU)
                && d.class() != 0xFE
                && serial
                    .map(|s| d.serial_number() == Some(s))
                    .unwrap_or(true)
        })
        .ok_or(DfuError::DeviceNotFound)?;

    let iface = dev
        .open()
        .wait()
        .map_err(|e| DfuError::UsbOpen(format!("open: {e}")))?
        .claim_interface(0)
        .wait()
        .map_err(|e| DfuError::UsbOpen(format!("claim interface: {e}")))?;

    let data = iface
        .control_in(
            ControlIn {
                control_type: ControlType::Vendor,
                recipient: Recipient::Device,
                request: RequestCode::GetInfo as u8,
                value: 0,
                index: 0,
                length: 12,
            },
            Duration::from_millis(500),
        )
        .wait()
        .map_err(|e| DfuError::Protocol(format!("GET_INFO: {e:?}")))?;

    if data.len() < 12 {
        return Err(DfuError::Protocol(format!(
            "GET_INFO short response: {} bytes",
            data.len()
        )));
    }
    let buf: [u8; 12] = data[..12].try_into().unwrap();
    let info = KCanDeviceInfo::from_bytes(&buf);
    Ok((info.fw_major, info.fw_minor, info.fw_patch))
}

/// Verify the Ed25519 signature in `signed_path` against `pubkey`, then
/// flash the firmware payload to the device in DFU mode.
///
/// `signed_path` is a `.bin.signed` file: raw binary followed by a 64-byte Ed25519 signature.
/// `pubkey` is the 32-byte verifying key (from `firmware/signing-pubkey.bin`).
/// `progress` is an optional callback invoked after each block with `(done, total)`.
pub fn flash_firmware(
    signed_path: &Path,
    pubkey: &[u8; 32],
    progress: Option<ProgressCallback>,
) -> Result<(), DfuError> {
    // ── Load and verify ──────────────────────────────────────────────────────
    let contents = std::fs::read(signed_path)?;
    if contents.len() <= SIG_SUFFIX_LEN {
        return Err(DfuError::Protocol(format!(
            "signed file too short: {} bytes",
            contents.len()
        )));
    }
    let payload_len = contents.len() - SIG_SUFFIX_LEN;
    let sig_bytes: &[u8; 64] = contents[payload_len..]
        .try_into()
        .map_err(|_| DfuError::Protocol("signature slice error".into()))?;
    let payload = &contents[..payload_len];

    let verifying_key = VerifyingKey::from_bytes(pubkey).map_err(|_| DfuError::SignatureInvalid)?;
    let signature = Signature::from_bytes(sig_bytes);
    // embassy-boot's verify_and_mark_updated pre-hashes firmware with SHA-512
    // before calling verify(), so we must verify against SHA-512(payload).
    let hash = Sha512::digest(payload);
    verifying_key
        .verify(hash.as_slice(), &signature)
        .map_err(|_| DfuError::SignatureInvalid)?;

    eprintln!(
        "DFU: signature OK — {} bytes firmware + 64 bytes sig to flash",
        payload.len()
    );

    // ── Open device via libusb (rusb) ─────────────────────────────────────────
    // nusb cannot claim interfaces on macOS when IOKit has bound them.
    // rusb wraps libusb which handles kernel driver detach transparently.
    let ctx = Context::new().map_err(|e| DfuError::UsbOpen(format!("libusb init: {e}")))?;
    let rusb_dev = {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let found = ctx
                .devices()
                .map_err(|e| DfuError::UsbOpen(format!("list devices: {e}")))?;
            let dev = found.iter().find(|d| {
                d.device_descriptor()
                    .map(|dd| dd.vendor_id() == KCAN_VID && dd.product_id() == KCAN_PID)
                    .unwrap_or(false)
            });
            if let Some(d) = dev {
                break d;
            }
            if std::time::Instant::now() >= deadline {
                return Err(DfuError::DeviceNotFound);
            }
            thread::sleep(Duration::from_millis(200));
        }
    };
    let handle = rusb_dev
        .open()
        .map_err(|e| DfuError::UsbOpen(format!("open: {e}")))?;
    // On Linux this detaches the kernel driver; on macOS libusb handles IOKit.
    let _ = handle.set_auto_detach_kernel_driver(true);
    handle
        .claim_interface(0)
        .map_err(|e| DfuError::UsbOpen(format!("claim interface: {e}")))?;

    // ── Check initial state ───────────────────────────────────────────────────
    let status = dfu_get_status(&handle)?;
    match status.state {
        DFU_ERROR => {
            dfu_clr_status(&handle)?;
            let status2 = dfu_get_status(&handle)?;
            if status2.state != DFU_IDLE {
                return Err(DfuError::Protocol(format!(
                    "unexpected state after CLRSTATUS: {}",
                    status2.state
                )));
            }
        }
        DFU_IDLE => {}
        s => {
            // Try to abort first, then clear
            let _ = dfu_abort(&handle);
            thread::sleep(Duration::from_millis(50));
            dfu_clr_status(&handle)?;
            let s2 = dfu_get_status(&handle)?;
            if s2.state != DFU_IDLE {
                return Err(DfuError::Protocol(format!(
                    "could not reach dfuIDLE, state={s}"
                )));
            }
        }
    }

    // ── Download blocks ───────────────────────────────────────────────────────
    // Send the full signed image (firmware + appended 64-byte signature) so
    // the device's embassy-usb-dfu can verify the signature in finish().
    let chunks: Vec<&[u8]> = contents.chunks(DFU_BLOCK_SIZE).collect();
    let total = chunks.len();
    for (block_num, chunk) in chunks.iter().enumerate() {
        dfu_dnload(&handle, block_num as u16, chunk)?;

        // Poll GETSTATUS until dfuDNLOAD_IDLE (or error)
        loop {
            let st = dfu_get_status(&handle)?;
            if st.status != STATUS_OK {
                return Err(DfuError::Protocol(format!(
                    "dfuERROR status=0x{:02x} state={} at block {block_num}",
                    st.status, st.state
                )));
            }
            match st.state {
                DFU_DNLOAD_IDLE => break,
                DFU_DNLOAD_SYNC | DFU_DNBUSY => {
                    thread::sleep(Duration::from_micros(st.poll_timeout_us as u64));
                }
                s => {
                    return Err(DfuError::Protocol(format!(
                        "unexpected state {s} at block {block_num}"
                    )));
                }
            }
        }

        if let Some(ref cb) = progress {
            cb(block_num + 1, total);
        }
    }

    // ── Zero-length DNLOAD signals end of transfer ─────────────────────────
    dfu_dnload(&handle, total as u16, &[])?;
    eprintln!("\nDFU: download complete — waiting for manifestation");

    // Poll until manifest complete
    loop {
        let st = dfu_get_status(&handle)?;
        if st.status != STATUS_OK {
            return Err(DfuError::Protocol(format!(
                "manifestation error status=0x{:02x} state={}",
                st.status, st.state
            )));
        }
        match st.state {
            DFU_MANIFEST_SYNC => {
                thread::sleep(Duration::from_micros(st.poll_timeout_us as u64));
            }
            DFU_MANIFEST => {
                // MANIFESTATION_TOLERANT: device resets on its own
                break;
            }
            DFU_IDLE => {
                // Some implementations go straight back to dfuIDLE
                break;
            }
            s => {
                return Err(DfuError::Protocol(format!(
                    "unexpected state {s} during manifestation"
                )));
            }
        }
    }

    eprintln!("DFU: firmware update complete — device rebooting");
    Ok(())
}

// ── DFU Class 1.1 low-level requests (rusb) ────────────────────────────────

struct DfuStatus {
    status: u8,
    poll_timeout_us: u32,
    state: u8,
}

const CTRL_TIMEOUT_MS: u64 = 5000;

fn dfu_get_status(handle: &DeviceHandle<Context>) -> Result<DfuStatus, DfuError> {
    let mut buf = [0u8; 6];
    let n = handle
        .read_control(
            0x21 | 0x80, // bmRequestType: class | interface | device-to-host
            DFU_GETSTATUS,
            0,
            0,
            &mut buf,
            Duration::from_millis(CTRL_TIMEOUT_MS),
        )
        .map_err(|e| DfuError::Protocol(format!("GETSTATUS: {e}")))?;
    if n < 6 {
        return Err(DfuError::Protocol(format!("GETSTATUS short: {n} bytes")));
    }
    let poll_timeout_us = u32::from_le_bytes([buf[1], buf[2], buf[3], 0]) * 1000;
    Ok(DfuStatus {
        status: buf[0],
        poll_timeout_us,
        state: buf[4],
    })
}

fn dfu_clr_status(handle: &DeviceHandle<Context>) -> Result<(), DfuError> {
    handle
        .write_control(
            0x21, // bmRequestType: class | interface | host-to-device
            DFU_CLRSTATUS,
            0,
            0,
            &[],
            Duration::from_millis(CTRL_TIMEOUT_MS),
        )
        .map_err(|e| DfuError::Protocol(format!("CLRSTATUS: {e}")))?;
    Ok(())
}

fn dfu_abort(handle: &DeviceHandle<Context>) -> Result<(), DfuError> {
    handle
        .write_control(
            0x21,
            DFU_ABORT,
            0,
            0,
            &[],
            Duration::from_millis(CTRL_TIMEOUT_MS),
        )
        .map_err(|e| DfuError::Protocol(format!("ABORT: {e}")))?;
    Ok(())
}

fn dfu_dnload(handle: &DeviceHandle<Context>, block_num: u16, data: &[u8]) -> Result<(), DfuError> {
    handle
        .write_control(
            0x21,
            DFU_DNLOAD,
            block_num,
            0,
            data,
            Duration::from_millis(CTRL_TIMEOUT_MS),
        )
        .map_err(|e| DfuError::Protocol(format!("DNLOAD block {block_num}: {e}")))?;
    Ok(())
}
