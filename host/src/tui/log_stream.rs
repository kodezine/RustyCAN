//! Stdout log-streaming mode for RustyCAN.
//!
//! Activated by `--log-to-stdout --config <FILE>`.  No GUI or TUI window is
//! opened; decoded CAN events are printed as timestamped plain-text lines to
//! standard output.  Output can be piped to a file or another tool:
//!
//! ```sh
//! rustycan --log-to-stdout --config my.json | tee capture.log
//! ```
//!
//! The function exits cleanly when:
//! - The CAN adapter thread disconnects (adapter error or cable pulled), or
//! - The user presses Ctrl-C (SIGINT).

use std::io::{self, Write};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use chrono::Utc;

use crate::app::{apply_event, AppState, CanEvent};
use crate::canopen::sdo::SdoDirection;

/// Load a config file and stream decoded CAN events to stdout.
///
/// Each event is printed as a single timestamped line using
/// `[HH:MM:SS.mmm] <type>  <details>` format.  Exits when the adapter thread
/// disconnects or Ctrl-C is received.
///
/// # Errors
/// Returns an error if the config file cannot be read/parsed, the session
/// fails to start, or a terminal I/O error occurs.
pub fn stream(config_path: &Path, _http_port: u16) -> io::Result<()> {
    let session_cfg = crate::gui::load_session_config(config_path, None)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let baud = session_cfg.baud;
    let (rx, _cmd_tx, node_labels, log_path) = crate::session::start(session_cfg)
        .map_err(|e| io::Error::other(format!("Session start failed: {e}")))?;

    let mut state = AppState::new(log_path.clone(), baud);
    state.init_nodes(&node_labels);

    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());

    // Print a startup banner so the user knows the session is live.
    writeln!(
        out,
        "# RustyCAN log-stream — connected  log_path={log_path}  nodes={}",
        node_labels
            .iter()
            .map(|(id, lbl)| format!("{id}={lbl}"))
            .collect::<Vec<_>>()
            .join(", ")
    )?;
    out.flush()?;

    // Install a SIGINT / Ctrl-C handler that sets a flag so we can exit cleanly.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let stop = stop.clone();
        // Ignore errors — the loop will still end on adapter disconnect.
        let _ = ctrlc::set_handler(move || {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        });
    }

    loop {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            writeln!(out, "# interrupted")?;
            break;
        }

        match rx.try_recv() {
            Ok(event) => {
                if let Some(line) = format_event(&event) {
                    let ts = Utc::now().format("%H:%M:%S%.3f").to_string();
                    writeln!(out, "[{ts}] {line}")?;
                    out.flush()?;
                }
                apply_event(&mut state, event);
            }
            Err(mpsc::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                writeln!(out, "# adapter disconnected")?;
                break;
            }
        }
    }

    out.flush()?;
    Ok(())
}

/// Format a [`CanEvent`] as a human-readable log line.
///
/// Returns `None` for events that produce no useful text output (e.g.
/// in-flight SDO pending markers or raw DBC signal updates).
fn format_event(event: &CanEvent) -> Option<String> {
    match event {
        CanEvent::Nmt { node_id, state } => Some(format!("NMT    node {node_id:3}  state {state}")),
        CanEvent::Sdo(entry) => {
            let dir = match entry.direction {
                SdoDirection::Read => "READ ",
                SdoDirection::Write => "WRITE",
            };
            let val = entry
                .value
                .as_ref()
                .map(|v| format!("{v}"))
                .or_else(|| entry.abort_code.map(|c| format!("ABORT 0x{c:08X}")))
                .unwrap_or_else(|| "pending".into());
            Some(format!(
                "SDO    node {:3}  {dir}  {:04X}:{:02X}  {} = {}",
                entry.node_id, entry.index, entry.subindex, entry.name, val
            ))
        }
        CanEvent::Pdo {
            node_id,
            cob_id,
            values,
        } => {
            let sigs: Vec<_> = values.iter().map(|v| format!("{v}")).collect();
            Some(format!(
                "PDO    node {:3}  cob 0x{cob_id:03X}  {}",
                node_id,
                sigs.join("  ")
            ))
        }
        CanEvent::AdapterError(msg) => Some(format!("ERROR  {msg}")),
        CanEvent::DbcLoaded(name) => Some(format!("DBC    loaded: {name}")),
        CanEvent::DbcSignal(signals) => {
            let parts: Vec<_> = signals
                .values
                .iter()
                .map(|s| format!("{}={:.4}", s.signal_name, s.physical))
                .collect();
            Some(format!(
                "DBC    id 0x{:03X}  {}  {}",
                signals.can_id,
                signals.message_name,
                parts.join("  ")
            ))
        }
        CanEvent::SdoPending { .. } => None,
        CanEvent::RawFrame { cob_id, data, port } => {
            let hex: String = data
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ");
            let ch = if *port == 0 { "FDCAN1" } else { "FDCAN2" };
            Some(format!(
                "RAW    [{cob_id:#05X}]  {ch}  dlc={dlc}  {hex}",
                dlc = data.len()
            ))
        }
    }
}
