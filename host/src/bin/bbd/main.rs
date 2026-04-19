//! `bbd` — BinaryBlockDownload firmware flashing tool.
//!
//! A pure-Rust, CANopen-protocol firmware update utility. It downloads a
//! binary block file to a CANopen bootloader node via SDO transfers, using
//! either a PEAK PCAN-USB adapter or a KCAN Dongle.
//!
//! This tool is a functional port of the `BinaryBlockDownload.c` C tool.
//!
//! Run `bbd --help` for full usage.

mod file;
mod sdo_client;
mod state_machine;

use std::path::PathBuf;
use std::process;
use std::time::Duration;

use clap::{ArgGroup, Parser};

use rustycan::adapters::{open_adapter, AdapterKind};

use sdo_client::{SdoClient, SdoClientConfig, SdocType};
use state_machine::{run_firmware_download, ActionType, DownloadConfig, Progress};

// ─── Version string ───────────────────────────────────────────────────────────

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (bbd)");

// ─── CLI definition ───────────────────────────────────────────────────────────

/// BinaryBlockDownload — CANopen firmware update tool.
///
/// Downloads a binary block file to a CANopen bootloader node via SDO transfers.
/// Supports PEAK PCAN-USB and KCAN dongle adapters.
///
/// Exit codes: 0 = success, non-zero = error.
#[derive(Parser, Debug)]
#[command(
    name = "bbd",
    version = VERSION,
    about = "CANopen firmware download tool — Rust port of BinaryBlockDownload",
    long_about = None,
)]
#[command(group(
    ArgGroup::new("action_flags")
        .args(["blupdate_app", "blupdate"])
        .multiple(false)
))]
struct Cli {
    /// Binary block file to download (.bin).
    #[arg(value_name = "FILE")]
    input_file: PathBuf,

    // ── Node / CAN settings ───────────────────────────────────────────────
    /// Target CANopen node ID (1–127).
    /// Accepts -n or -N (matching the C BinaryBlockDownload tool convention).
    #[arg(
        short = 'n',
        short_alias = 'N',
        long,
        value_name = "NUM",
        default_value_t = 1
    )]
    node_id: u8,

    /// Application program slot number (subindex of 0x1F50/0x1F51/0x1F57).
    /// Accepts -p or -P (matching the C BinaryBlockDownload tool convention).
    #[arg(
        short = 'p',
        short_alias = 'P',
        long,
        value_name = "NUM",
        default_value_t = 1
    )]
    program_number: u8,

    /// SDO response timeout in milliseconds.
    #[arg(long, value_name = "MS", default_value_t = 500)]
    timeout: u64,

    /// Poll delay between flash-status reads, in 100 µs units (default 500 = 50 ms).
    #[arg(long, value_name = "100US", default_value_t = 500)]
    delay: u32,

    /// Delay before checking if the application started, in 100 µs units (default 20000 = 2 s).
    #[arg(long, value_name = "100US", default_value_t = 20000)]
    delay_check_app: u32,

    /// Delay before checking if the bootloader re-entered, in 100 µs units (default 20000 = 2 s).
    #[arg(long, value_name = "100US", default_value_t = 20000)]
    delay_check_bl: u32,

    /// SDO transfer mode for large block downloads: 0 = segmented (default), 2 = block.
    #[arg(long, value_name = "TYPE", default_value_t = 0)]
    sdoc_type: u8,

    /// Maximum retry count for flash-BUSY polling.
    #[arg(long, value_name = "NUM", default_value_t = 200000)]
    repeats: u32,

    /// Maximum retry count for flash-CRC-BUSY polling.
    #[arg(long, value_name = "NUM", default_value_t = 10000)]
    repeats_crc: u32,

    /// SDO request (master→node) COB-ID base (default 0x600).
    #[arg(long, value_name = "HEX", default_value = "0x600")]
    tx_baseid: String,

    /// SDO response (node→master) COB-ID base (default 0x580).
    #[arg(long, value_name = "HEX", default_value = "0x580")]
    rx_baseid: String,

    /// Vendor ID to validate (0 = skip check).
    #[arg(long, value_name = "HEX", default_value = "0x0")]
    vendor_id: String,

    /// Product code to validate (0 = skip check).
    #[arg(long, value_name = "HEX", default_value = "0x0")]
    product_code: String,

    // ── Progress / multi-step ─────────────────────────────────────────────
    /// Total number of steps in a multi-step flash sequence (for progress display).
    #[arg(long, value_name = "NUM", default_value_t = 0)]
    total_steps: u8,

    /// Current step number in a multi-step flash sequence.
    #[arg(long, value_name = "NUM", default_value_t = 0)]
    current_step: u8,

    // ── Action flags ──────────────────────────────────────────────────────
    /// Load the bootloader-update application (mutually exclusive with --blupdate).
    #[arg(long)]
    blupdate_app: bool,

    /// Update the bootloader itself via the blupdate-app (mutually exclusive with --blupdate-app).
    #[arg(long)]
    blupdate: bool,

    // ── Adapter selection ─────────────────────────────────────────────────
    /// CAN adapter backend: peak or kcan.
    #[arg(long, value_name = "ADAPTER", default_value = "peak")]
    adapter: String,

    /// Adapter port / channel (PEAK: channel number e.g. "1"; KCAN: ignored if --kcan-serial is set).
    #[arg(long, value_name = "PORT", default_value = "1")]
    port: String,

    /// CAN bus baud rate in bits per second (e.g. 500000).
    #[arg(long, value_name = "BPS", default_value_t = 500_000)]
    baud: u32,

    /// KCAN dongle USB serial number (optional; uses first found if omitted).
    #[arg(long, value_name = "SERIAL")]
    kcan_serial: Option<String>,
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Parse a string that may be decimal (`"1234"`) or `0x`-prefixed hex (`"0x600"`).
fn parse_u32_auto(s: &str) -> Result<u32, String> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).map_err(|e| format!("invalid hex value {s:?}: {e}"))
    } else {
        s.parse::<u32>()
            .map_err(|e| format!("invalid decimal value {s:?}: {e}"))
    }
}

// ─── Entry point ─────────────────────────────────────────────────────────────

fn main() {
    let cli = Cli::parse();

    // ── Print version banner ──────────────────────────────────────────────────
    println!("--------------------------------------------------------------------");
    println!("BinaryBlockDownload (bbd) v{}", VERSION);
    println!("--------------------------------------------------------------------");

    // ── Validate node ID ─────────────────────────────────────────────────────
    if cli.node_id == 0 || cli.node_id > 127 {
        eprintln!("Error: node ID must be in range 1–127, got {}", cli.node_id);
        process::exit(1);
    }

    // ── Parse hex parameters ──────────────────────────────────────────────────
    let tx_baseid = match parse_u32_auto(&cli.tx_baseid) {
        Ok(v) => v as u16,
        Err(e) => {
            eprintln!("Error: --tx-baseid: {e}");
            process::exit(1);
        }
    };
    let rx_baseid = match parse_u32_auto(&cli.rx_baseid) {
        Ok(v) => v as u16,
        Err(e) => {
            eprintln!("Error: --rx-baseid: {e}");
            process::exit(1);
        }
    };
    let vendor_id = match parse_u32_auto(&cli.vendor_id) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: --vendor-id: {e}");
            process::exit(1);
        }
    };
    let product_code = match parse_u32_auto(&cli.product_code) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: --product-code: {e}");
            process::exit(1);
        }
    };

    // ── SDO transfer mode ─────────────────────────────────────────────────────
    let sdoc_type = match SdocType::from_u8(cli.sdoc_type) {
        Some(t) => t,
        None => {
            eprintln!(
                "Error: --sdoc-type must be 0 (segmented) or 2 (block), got {}",
                cli.sdoc_type
            );
            process::exit(1);
        }
    };

    // ── Action type ───────────────────────────────────────────────────────────
    let action = if cli.blupdate_app {
        println!("Action: LOAD BOOTLOADER UPDATE APPLICATION");
        ActionType::LoadBlupdateApp
    } else if cli.blupdate {
        println!("Action: UPDATE BOOTLOADER");
        ActionType::UpdateBootloader
    } else {
        ActionType::UpdateFirmware
    };

    // ── Adapter kind ──────────────────────────────────────────────────────────
    let adapter_kind = match cli.adapter.to_lowercase().as_str() {
        "peak" => AdapterKind::Peak,
        "kcan" => AdapterKind::KCan {
            serial: cli.kcan_serial.clone(),
        },
        other => {
            eprintln!("Error: unknown adapter {other:?}. Use 'peak' or 'kcan'.");
            process::exit(1);
        }
    };

    println!("Adapter : {} (port {})", cli.adapter, cli.port);
    println!("Node ID : {}", cli.node_id);
    println!("Baud    : {} bps", cli.baud);
    println!("File    : {}", cli.input_file.display());
    println!(
        "SDO mode: {}",
        if sdoc_type == SdocType::Block {
            "block"
        } else {
            "segmented"
        }
    );
    println!("--------------------------------------------------------------------");

    // ── Open adapter ──────────────────────────────────────────────────────────
    let adapter = match open_adapter(&adapter_kind, &cli.port, cli.baud, false) {
        Ok(a) => {
            println!("Adapter opened: {}", a.name());
            a
        }
        Err(e) => {
            eprintln!("Error: failed to open adapter: {e}");
            process::exit(1);
        }
    };

    // ── Build SDO client ──────────────────────────────────────────────────────
    let sdo_cfg = SdoClientConfig {
        node_id: cli.node_id,
        timeout: Duration::from_millis(cli.timeout),
        tx_base_id: tx_baseid,
        rx_base_id: rx_baseid,
    };
    let mut client = SdoClient::new(adapter, sdo_cfg);

    // ── Build download config ─────────────────────────────────────────────────
    let dl_cfg = DownloadConfig {
        program_number: cli.program_number,
        vendor_id,
        product_code,
        max_retries_busy: cli.repeats,
        max_retries_crc: cli.repeats_crc,
        poll_delay_100us: cli.delay,
        delay_check_app_100us: cli.delay_check_app,
        delay_check_bl_100us: cli.delay_check_bl,
        action,
        sdoc_type,
        total_steps: cli.total_steps,
        current_step: cli.current_step,
    };

    // ── Progress callback ─────────────────────────────────────────────────────
    let mut total_blocks_done = 0u32;
    let mut progress_cb = |p: Progress| match p {
        Progress::State(s) => println!("[BBD] State: {s}"),
        Progress::BlockDownloaded {
            block_num,
            bytes_done,
            total_bytes,
        } => {
            total_blocks_done += 1;
            let pct = (bytes_done * 100)
                .checked_div(total_bytes)
                .unwrap_or(0)
                .min(100);
            println!(
                "[BBD] Block {:>4} downloaded  ({} %)  [{} / {} bytes]",
                block_num, pct, bytes_done, total_bytes
            );
        }
        Progress::Percent(p) => println!("[BBD] Progress: {p} %"),
    };

    // ── Run state machine ─────────────────────────────────────────────────────
    match run_firmware_download(&mut client, &dl_cfg, &cli.input_file, &mut progress_cb) {
        Ok(()) => {
            println!("--------------------------------------------------------------------");
            println!("Firmware download SUCCESSFUL ({total_blocks_done} block(s) written).");
            println!("--------------------------------------------------------------------");
            process::exit(0);
        }
        Err(e) => {
            eprintln!("--------------------------------------------------------------------");
            eprintln!("Firmware download FAILED: {e}");
            eprintln!("--------------------------------------------------------------------");
            process::exit(1);
        }
    }
}
