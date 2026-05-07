// Hide the console window on release builds when launched from Explorer / shortcuts.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use clap::Parser;

/// RustyCAN — CAN-bus monitor and CANopen analyser.
#[derive(Parser)]
#[command(version, about)]
struct CliArgs {
    /// Path to a JSON configuration file.
    ///
    /// When supplied the application skips the connect form and starts the CAN
    /// session automatically using the settings in the file.  The file follows
    /// the same schema that RustyCAN writes to its app-data directory; see the
    /// bundled `config.example.json` for a fully annotated example.
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Port for the live HTML dashboard (`http://127.0.0.1:<port>/`).
    ///
    /// Overrides the `http_port` field in the JSON config file when both are
    /// supplied.  Defaults to 7878 when absent from both the CLI and the file.
    #[arg(long, value_name = "PORT")]
    http_port: Option<u16>,

    /// Start RustyCAN as a full-screen terminal UI instead of opening a GUI window.
    ///
    /// Requires `--config` to be supplied.  The TUI displays live NMT, PDO, and
    /// SDO panels.  Press `n` to send an NMT command, `s` for SDO read, `w` for
    /// SDO write, `L` to toggle the event-log panel, and `q` / Ctrl-C to quit.
    #[arg(long)]
    tui: bool,

    /// Print decoded CAN events as timestamped text lines to stdout and exit
    /// when the adapter disconnects or Ctrl-C is pressed.
    ///
    /// Requires `--config` to be supplied.  No GUI or TUI window is opened;
    /// output can be piped directly to a file or another tool.
    #[arg(long)]
    log_to_stdout: bool,

    /// Flash a signed KCAN dongle firmware image over USB DFU.
    ///
    /// The file must be a `.bin.signed` produced by `sign-firmware`
    /// (raw binary followed by a 64-byte Ed25519 signature).  The host verifies
    /// the signature against the embedded public key before initiating DFU.
    ///
    /// Flow:
    ///   1. Send DFU_DETACH to the running KCAN app (triggers reboot into bootloader)
    ///   2. Wait up to 15 s for the device to re-enumerate in DFU mode
    ///   3. Verify Ed25519 signature
    ///   4. Stream firmware blocks via DFU_DNLOAD
    ///   5. Device verifies + flashes + reboots into new app automatically
    ///
    /// Use `--kcan-serial` to target a specific dongle when multiple are attached.
    #[arg(long, value_name = "SIGNED_BIN")]
    dfu_update: Option<PathBuf>,

    /// USB serial number of the KCAN dongle to target (for --dfu-update).
    ///
    /// When omitted the first KCAN dongle found is used.
    #[arg(long, value_name = "SERIAL")]
    kcan_serial: Option<String>,
}

fn main() {
    let args = CliArgs::parse();

    // --dfu-update without --tui and without --config: run DFU immediately, then exit.
    // With --config (GUI mode): the GUI shows a firmware update banner and [Update Now] button.
    // With --tui: launch TUI first; user confirms with U→y inside the TUI.
    if let Some(ref signed_path) = args.dfu_update {
        if !args.tui && args.config.is_none() {
            run_dfu_update(signed_path, args.kcan_serial.as_deref());
            return;
        }
    }

    // Validate flags that require --config.
    if args.tui && args.config.is_none() {
        eprintln!("error: --tui requires --config <FILE>");
        std::process::exit(1);
    }
    if args.log_to_stdout && args.config.is_none() {
        eprintln!("error: --log-to-stdout requires --config <FILE>");
        std::process::exit(1);
    }

    // Resolve the effective HTTP port:
    //   1. CLI --http-port (highest priority)
    //   2. "http_port" field inside the JSON config file
    //   3. Hard-coded default 7878
    let effective_port: u16 = args.http_port.unwrap_or_else(|| {
        args.config
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| {
                serde_json::from_str::<serde_json::Value>(&s)
                    .ok()
                    .and_then(|v| v.get("http_port").and_then(|x| x.as_u64()))
                    .and_then(|n| u16::try_from(n).ok())
            })
            .unwrap_or(7878)
    });

    if args.log_to_stdout {
        // SAFETY: config is Some — validated above.
        let path = args.config.as_deref().unwrap();
        if let Err(e) = rustycan::tui::log_stream::stream(path, effective_port) {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    } else if args.tui {
        let path = args.config.as_deref().unwrap();
        let dfu_path = args.dfu_update.as_deref();
        match rustycan::tui::run_from_config(path, effective_port, dfu_path) {
            Ok(rustycan::tui::TuiExitReason::DfuUpdate) => {
                // User confirmed firmware update from inside the TUI.
                if let Some(ref signed_path) = args.dfu_update {
                    run_dfu_update(signed_path, args.kcan_serial.as_deref());
                }
            }
            Ok(rustycan::tui::TuiExitReason::Quit) => {}
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    } else {
        let dfu_path = if !args.tui { args.dfu_update } else { None };
        if let Err(e) = rustycan::gui::run(args.config, effective_port, dfu_path) {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

/// Run a non-interactive DFU firmware update and exit.
///
/// Steps:
///   1. Query device firmware version via GET_INFO and compare to bundled version.
///   2. Prompt `y/N` if already up-to-date (skip prompt when stdin is not a tty).
///   3. Send DFU_DETACH to the running app (it resets into the bootloader).
///   4. Wait up to 15 s for the bootloader to enumerate in DFU mode.
///   5. Verify the Ed25519 signature embedded in `signed_path`.
///   6. Download the firmware payload via DFU Class 1.1 block transfers (with progress bar).
///   7. Device verifies, flashes, and reboots into the new app automatically.
fn run_dfu_update(signed_path: &std::path::Path, serial: Option<&str>) {
    use rustycan::adapters::kcan::KCanAdapter;
    use rustycan::dfu::{
        flash_firmware, get_device_firmware_version, wait_for_dfu_device, DfuError,
    };
    use std::time::Duration;

    // ── Step 1: Query device firmware version ────────────────────────────────
    let bundled = rustycan::bundled_firmware_version();
    let mut app_was_running = false;
    match get_device_firmware_version(serial) {
        Ok((dev_maj, dev_min, dev_pat)) => {
            app_was_running = true;
            println!("[DFU] Device firmware: v{dev_maj}.{dev_min}.{dev_pat}");
            if let Some((b_maj, b_min, b_pat)) = bundled {
                if (dev_maj, dev_min, dev_pat) == (b_maj, b_min, b_pat) {
                    println!(
                        "[DFU] Device is already at v{b_maj}.{b_min}.{b_pat} — firmware is up to date"
                    );
                    // Prompt only when stdin is an interactive terminal.
                    if atty::is(atty::Stream::Stdin) {
                        print!("[DFU] Update anyway? [y/N] ");
                        use std::io::Write;
                        let _ = std::io::stdout().flush();
                        let mut line = String::new();
                        let _ = std::io::stdin().read_line(&mut line);
                        if !matches!(line.trim(), "y" | "Y") {
                            println!("[DFU] Aborted.");
                            return;
                        }
                    }
                } else {
                    println!(
                        "[DFU] Updating: v{dev_maj}.{dev_min}.{dev_pat} → v{b_maj}.{b_min}.{b_pat}"
                    );
                }
            } else {
                println!("[DFU] Bundled version unknown (dev build) — proceeding");
            }
        }
        Err(DfuError::DeviceNotFound) => {
            // App not found — the device may already be in DFU bootloader mode
            // (e.g. mid-swap after a previous update) or may not be connected.
            // Skip DFU_DETACH: there is no running app to detach from.
            eprintln!("[DFU] App not running — checking for existing DFU bootloader");
        }
        Err(e) => {
            // Other error (USB open failure, etc.) — proceed and try DFU_DETACH anyway.
            eprintln!("[DFU] Could not query device version: {e} — proceeding");
            app_was_running = true; // attempt detach
        }
    }

    // ── Step 2: Send DFU_DETACH (only when the app was found running) ─────────
    if app_was_running {
        println!("[DFU] Sending DFU_DETACH to KCAN dongle...");
        match KCanAdapter::enter_dfu_mode(serial) {
            Ok(()) => println!("[DFU] DFU_DETACH sent — device rebooting into bootloader"),
            Err(e) => {
                // Device may have already reset before ACKing — treat as non-fatal.
                eprintln!(
                    "[DFU] enter_dfu_mode: {e} (device may have reset before ACK — continuing)"
                );
            }
        }
    }

    // ── Step 3: Wait for DFU bootloader ──────────────────────────────────────
    println!("[DFU] Waiting for DFU bootloader to enumerate (up to 15 s)...");
    let _dev = match wait_for_dfu_device(Duration::from_secs(15)) {
        Ok(d) => {
            println!(
                "[DFU] Bootloader found: {}",
                d.product_string().unwrap_or("KCAN DFU")
            );
            d
        }
        Err(DfuError::DeviceNotFound) => {
            eprintln!("[DFU] error: KCAN device did not appear in DFU mode within 15 s");
            eprintln!(
                "       • Is the bootloader flashed? (probe-rs download --base-address 0x08000000)"
            );
            eprintln!("       • Is the correct dongle connected?");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("[DFU] error: {e}");
            std::process::exit(1);
        }
    };

    // ── Step 4: Verify + flash with progress bar ──────────────────────────────
    println!(
        "[DFU] Verifying signature and flashing {}...",
        signed_path.display()
    );
    let progress: rustycan::dfu::ProgressCallback = Box::new(|done, total| {
        let pct = done * 100 / total;
        eprint!("\r[DFU] Downloading [{pct:>3}%] block {done}/{total}   ");
        if done == total {
            eprintln!();
        }
    });
    match flash_firmware(signed_path, &rustycan::KCAN_SIGNING_PUBKEY, Some(progress)) {
        Ok(()) => {
            println!("[DFU] Firmware update complete — device is rebooting into new app");
        }
        Err(DfuError::SignatureInvalid) => {
            eprintln!("[DFU] error: Ed25519 signature verification failed");
            eprintln!("       Ensure the binary was signed with the key matching firmware/signing-pubkey.bin");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("[DFU] error: {e}");
            std::process::exit(1);
        }
    }
}
