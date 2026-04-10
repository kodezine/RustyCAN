//! Terminal UI (TUI) frontend for RustyCAN.
//!
//! Activated by passing `--tui --config <FILE>` on the command line.  The TUI
//! renders live NMT, PDO, SDO, and event-log panels using [ratatui] and
//! [crossterm].  It also supports interactive NMT and SDO commands typed
//! directly in the terminal — no GUI window is required.
//!
//! # Key bindings (Normal mode)
//!
//! | Key       | Action                              |
//! |-----------|-------------------------------------|
//! | `n`       | Enter NMT command input             |
//! | `s`       | Enter SDO read input                |
//! | `w`       | Enter SDO write input               |
//! | `L`       | Toggle the event-log panel          |
//! | `q` / `Q` | Quit                                |
//! | Ctrl-C    | Quit                                |
//!
//! # Command formats
//!
//! **NMT:** `<node_id> <command>`
//! - Commands: `start`, `stop`, `pre_op`, `reset_node`, `reset_comm`
//! - `node_id` 0 broadcasts to all nodes.
//! - Example: `1 start`
//!
//! **SDO read:** `<node> <index_hex> <sub>`
//! - Example: `1 1000 0`
//!
//! **SDO write:** `<node> <index_hex> <sub> <hex_value>`
//! - Example: `1 6040 0 0006`

pub mod log_stream;
mod widgets;

use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    Terminal,
};

use crate::app::{AppState, CanEvent};
use crate::canopen::nmt::NmtCommand;
use crate::canopen::sdo::SdoTransferMode;
use crate::session::CanCommand;

// ─── Stderr guard ─────────────────────────────────────────────────────────────

/// Redirects stderr (fd 2) to a log file for the lifetime of `self`.
///
/// Any `eprintln!`, adapter-library output, or panic messages produced while
/// the TUI is active are written to the file instead of the terminal, so they
/// cannot corrupt the ratatui display.  The original stderr is restored when
/// the guard is dropped (i.e. after the TUI exits and the terminal is already
/// back in normal mode).
///
/// On non-Unix platforms the guard is a no-op; the field is present so that
/// call sites compile unchanged on all targets.
pub struct StderrGuard {
    #[cfg(unix)]
    saved_fd: std::os::fd::OwnedFd,
}

impl StderrGuard {
    /// Redirect stderr to `path`.  Silently returns a no-op guard if the file
    /// cannot be opened or the `dup` system call fails.
    #[allow(unused_variables)]
    pub fn redirect(path: &Path) -> Self {
        #[cfg(unix)]
        {
            use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
            use std::os::unix::fs::OpenOptionsExt;

            // Open (or create/truncate) the stderr capture file.
            let Ok(file) = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)
            else {
                // Can't open the file — return no-op guard.
                return StderrGuard {
                    saved_fd: unsafe { OwnedFd::from_raw_fd(2) },
                };
            };

            // Save current stderr fd so we can restore it on drop.
            let saved_raw = unsafe { libc::dup(2) };
            if saved_raw < 0 {
                return StderrGuard {
                    saved_fd: unsafe { OwnedFd::from_raw_fd(2) },
                };
            }

            // Point fd 2 at the log file.  The file's OwnedFd closes its own
            // fd immediately after dup2 — fd 2 is all we need.
            let file_fd = file.into_raw_fd();
            unsafe { libc::dup2(file_fd, 2) };
            unsafe { libc::close(file_fd) };

            StderrGuard {
                saved_fd: unsafe { OwnedFd::from_raw_fd(saved_raw) },
            }
        }
        #[cfg(not(unix))]
        {
            StderrGuard {}
        }
    }
}

impl Drop for StderrGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // Restore the saved stderr fd back to position 2.
            unsafe { libc::dup2(self.saved_fd.as_raw_fd(), 2) };
            // saved_fd is dropped here, closing the saved copy.
        }
    }
}

// ─── Public types ─────────────────────────────────────────────────────────────

/// The kind of command being typed in input mode.
#[derive(Debug, Clone, PartialEq)]
pub enum InputKind {
    /// NMT master command.  Format: `<node_id> <command>`.
    Nmt,
    /// SDO upload (master reads from node).  Format: `<node> <index_hex> <sub>`.
    SdoRead,
    /// SDO download (master writes to node).  Format: `<node> <index_hex> <sub> <hex_value>`.
    SdoWrite,
}

/// Current interaction mode of the TUI.
#[derive(Debug, Clone)]
pub enum TuiMode {
    /// Normal monitoring mode; key bindings shown in the command bar.
    Normal,
    /// The user is typing a command.  `buf` holds the characters typed so far.
    Input { kind: InputKind, buf: String },
}

// ─── Entry points ─────────────────────────────────────────────────────────────

/// Load a config file and run the full-screen TUI.
///
/// This is the entry point called from `main` when `--tui` is supplied.
/// It loads the [`crate::session::SessionConfig`] from the JSON file at
/// `config_path`, starts the CAN session, then hands off to [`run`].
///
/// Stderr is redirected to a companion `.stderr` file alongside the JSONL log
/// for the duration of the TUI so that any background `eprintln!` or adapter
/// library output cannot corrupt the terminal display.
///
/// # Errors
/// Returns an [`io::Error`] on terminal setup failure or if the config file
/// cannot be read / parsed (wrapped as `InvalidData`).
pub fn run_from_config(config_path: &Path, _http_port: u16) -> io::Result<()> {
    // Build SessionConfig from the JSON file.
    let session_cfg = crate::gui::load_session_config(config_path, None)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    // Derive a stderr capture path: same stem as the JSONL log, `.stderr` extension.
    // Falls back to a path next to the config file if log_path is a bare filename.
    let stderr_log_path = {
        let log = std::path::Path::new(&session_cfg.log_path);
        let stem = log
            .file_stem()
            .unwrap_or_else(|| std::ffi::OsStr::new("rustycan"));
        // Prefer the log file's parent directory; fall back to the config file's dir.
        let dir = log
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(|| config_path.parent())
            .unwrap_or_else(|| std::path::Path::new("."));
        dir.join(format!("{}.stderr", stem.to_string_lossy()))
    };

    // Redirect stderr to the capture file for the entire TUI session.
    // The guard restores the original stderr when it is dropped (after TUI exits).
    let _stderr_guard = StderrGuard::redirect(&stderr_log_path);

    let baud = session_cfg.baud;
    let (rx, cmd_tx, node_labels, log_path) = crate::session::start(session_cfg)
        .map_err(|e| io::Error::other(format!("Session start failed: {e}")))?;

    let state = AppState::new(log_path, baud);
    run(rx, cmd_tx, state, node_labels)
}

/// Run the TUI event loop with an already-started CAN session.
///
/// Exits when the user presses `q` / `Q` / Ctrl-C, or when the adapter
/// thread disconnects.  The terminal is fully restored on exit even if a
/// panic or error occurs in the event loop.
///
/// # Arguments
/// - `rx`    — receiver for decoded [`CanEvent`]s from the session thread.
/// - `cmd_tx` — sender for [`CanCommand`]s back to the session thread.
/// - `state` — initial (empty) [`AppState`]; nodes are pre-seeded inside.
/// - `nodes` — `(node_id, label)` pairs from the session handshake.
pub fn run(
    rx: mpsc::Receiver<CanEvent>,
    cmd_tx: mpsc::Sender<CanCommand>,
    state: AppState,
    nodes: Vec<(u8, String)>,
) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, rx, cmd_tx, state, nodes);

    // Always restore the terminal, even on error.
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

// ─── Internal event loop ──────────────────────────────────────────────────────

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    rx: mpsc::Receiver<CanEvent>,
    cmd_tx: mpsc::Sender<CanCommand>,
    mut state: AppState,
    nodes: Vec<(u8, String)>,
) -> io::Result<()> {
    state.init_nodes(&nodes);

    let mut mode = TuiMode::Normal;
    let mut log_entries: VecDeque<String> = VecDeque::with_capacity(512);
    let mut show_log = true;
    let tick = Duration::from_millis(100);

    loop {
        // Drain all pending decoded CAN events (non-blocking); collect log lines.
        let alive = drain_events_with_log(&mut state, &rx, &mut log_entries);

        // Render current frame.
        draw(terminal, &state, &mode, &log_entries, show_log)?;

        if !alive {
            // Adapter thread died — show the final frame for a moment then exit.
            std::thread::sleep(Duration::from_millis(500));
            return Ok(());
        }

        // Poll keyboard for up to `tick` duration.
        if event::poll(tick)? {
            if let Event::Key(key) = event::read()? {
                match &mut mode {
                    TuiMode::Normal => match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(()),
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(());
                        }
                        KeyCode::Char('n') => {
                            mode = TuiMode::Input {
                                kind: InputKind::Nmt,
                                buf: String::new(),
                            };
                        }
                        KeyCode::Char('s') => {
                            mode = TuiMode::Input {
                                kind: InputKind::SdoRead,
                                buf: String::new(),
                            };
                        }
                        KeyCode::Char('w') => {
                            mode = TuiMode::Input {
                                kind: InputKind::SdoWrite,
                                buf: String::new(),
                            };
                        }
                        KeyCode::Char('L') | KeyCode::Char('l') => {
                            show_log = !show_log;
                        }
                        _ => {}
                    },
                    TuiMode::Input { kind, buf } => match key.code {
                        KeyCode::Esc => {
                            mode = TuiMode::Normal;
                        }
                        KeyCode::Enter => {
                            let input = buf.trim().to_string();
                            let kind_clone = kind.clone();
                            mode = TuiMode::Normal;
                            if let Some(cmd) = parse_command(&kind_clone, &input) {
                                // Non-blocking send; ignore if the session thread is gone.
                                let _ = cmd_tx.send(cmd);
                            }
                        }
                        KeyCode::Backspace => {
                            buf.pop();
                        }
                        KeyCode::Char(c) => {
                            buf.push(c);
                        }
                        _ => {}
                    },
                }
            }
        }
    }
}

// ─── Rendering ───────────────────────────────────────────────────────────────

fn draw(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &AppState,
    mode: &TuiMode,
    log_entries: &VecDeque<String>,
    show_log: bool,
) -> io::Result<()> {
    terminal.draw(|f| {
        let size = f.area();

        let nmt_height = (state.node_map.len() as u16 + 4)
            .min(size.height / 3)
            .max(6);
        let cmd_height: u16 = 1;
        let stats_height: u16 = 1;

        let log_height: u16 = if show_log {
            (size.height / 4).clamp(4, 12)
        } else {
            0
        };

        let remaining = size
            .height
            .saturating_sub(nmt_height + cmd_height + stats_height + log_height);
        let sdo_height = (remaining / 2).max(3);
        let pdo_height = remaining.saturating_sub(sdo_height).max(3);

        let mut constraints = vec![
            Constraint::Length(nmt_height),
            Constraint::Length(pdo_height),
            Constraint::Length(sdo_height),
        ];
        if show_log {
            constraints.push(Constraint::Length(log_height));
        }
        constraints.push(Constraint::Length(stats_height));
        constraints.push(Constraint::Length(cmd_height));

        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(size);

        let mut idx = 0;
        widgets::render_nmt_panel(f, state, vertical[idx]);
        idx += 1;
        widgets::render_pdo_panel(f, state, vertical[idx]);
        idx += 1;
        widgets::render_sdo_log(f, state, vertical[idx]);
        idx += 1;
        if show_log {
            widgets::render_log_panel(f, log_entries, vertical[idx]);
            idx += 1;
        }
        widgets::render_stats_bar(f, state, vertical[idx]);
        idx += 1;
        widgets::render_command_bar(f, mode, vertical[idx]);
    })?;
    Ok(())
}

// ─── Event draining with log ──────────────────────────────────────────────────

/// Drain all pending [`CanEvent`]s, apply them to `state`, and append a
/// human-readable one-liner to `log_entries` for each event.
///
/// Returns `false` when the adapter thread has disconnected (sender dropped).
fn drain_events_with_log(
    state: &mut AppState,
    rx: &mpsc::Receiver<CanEvent>,
    log_entries: &mut VecDeque<String>,
) -> bool {
    use chrono::Utc;
    const LOG_CAP: usize = 512;

    loop {
        match rx.try_recv() {
            Ok(event) => {
                let line = event_to_log_line(&event);
                crate::app::apply_event(state, event);
                if let Some(line) = line {
                    if log_entries.len() >= LOG_CAP {
                        log_entries.pop_front();
                    }
                    let ts = Utc::now().format("%H:%M:%S%.3f").to_string();
                    log_entries.push_back(format!("[{ts}] {line}"));
                }
            }
            Err(mpsc::TryRecvError::Empty) => return true,
            Err(mpsc::TryRecvError::Disconnected) => return false,
        }
    }
}

fn event_to_log_line(event: &CanEvent) -> Option<String> {
    match event {
        CanEvent::Nmt { node_id, state } => Some(format!("NMT  node {node_id}  → {state}")),
        CanEvent::Sdo(entry) => {
            let dir = match entry.direction {
                crate::canopen::sdo::SdoDirection::Read => "READ",
                crate::canopen::sdo::SdoDirection::Write => "WRITE",
            };
            let val = entry
                .value
                .as_ref()
                .map(|v| format!("{v}"))
                .or_else(|| entry.abort_code.map(|c| format!("ABORT 0x{c:08X}")))
                .unwrap_or_else(|| "pending".into());
            Some(format!(
                "SDO  node {}  {dir}  {:04X}:{:02X}  {} = {}",
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
                "PDO  node {node_id}  cob 0x{cob_id:03X}  {}",
                sigs.join(", ")
            ))
        }
        CanEvent::AdapterError(msg) => Some(format!("ERROR  {msg}")),
        CanEvent::DbcLoaded(name) => Some(format!("DBC loaded: {name}")),
        CanEvent::DbcSignal(_) | CanEvent::SdoPending { .. } => None,
    }
}

// ─── Command parsing ──────────────────────────────────────────────────────────

/// Parse a typed command string and return the corresponding [`CanCommand`].
///
/// Returns `None` on parse error (the TUI just silently discards invalid input;
/// a future improvement could show an inline error in the command bar).
fn parse_command(kind: &InputKind, input: &str) -> Option<CanCommand> {
    let parts: Vec<&str> = input.split_whitespace().collect();
    match kind {
        InputKind::Nmt => {
            if parts.len() < 2 {
                return None;
            }
            let node: u8 = parts[0].parse().ok()?;
            let cmd = parse_nmt_command(parts[1])?;
            Some(CanCommand::SendNmt {
                command: cmd,
                target_node: node,
            })
        }
        InputKind::SdoRead => {
            if parts.len() < 3 {
                return None;
            }
            let node: u8 = parts[0].parse().ok()?;
            let index = u16::from_str_radix(parts[1].trim_start_matches("0x"), 16).ok()?;
            let subindex: u8 = parts[2].parse().ok()?;
            Some(CanCommand::SdoRead {
                node_id: node,
                index,
                subindex,
                mode: SdoTransferMode::Auto,
            })
        }
        InputKind::SdoWrite => {
            if parts.len() < 4 {
                return None;
            }
            let node: u8 = parts[0].parse().ok()?;
            let index = u16::from_str_radix(parts[1].trim_start_matches("0x"), 16).ok()?;
            let subindex: u8 = parts[2].parse().ok()?;
            let hex_str = parts[3].trim_start_matches("0x");
            // Parse as u64 then convert to LE bytes, trimming zero-padding.
            let val_u64 = u64::from_str_radix(hex_str, 16).ok()?;
            let byte_len = hex_str.len().div_ceil(2).clamp(1, 8);
            let data: Vec<u8> = val_u64.to_le_bytes()[..byte_len].to_vec();
            Some(CanCommand::SdoWrite {
                node_id: node,
                index,
                subindex,
                data,
                mode: SdoTransferMode::Auto,
            })
        }
    }
}

/// Parse a human-typed NMT command name to [`NmtCommand`].
///
/// Accepts abbreviated and case-insensitive spellings:
/// `start`, `stop`, `pre_op` / `preop` / `pre_operational`,
/// `reset_node` / `reset`, `reset_comm` / `reset_communication`.
fn parse_nmt_command(s: &str) -> Option<NmtCommand> {
    match s.to_ascii_lowercase().as_str() {
        "start" => Some(NmtCommand::StartRemoteNode),
        "stop" => Some(NmtCommand::StopRemoteNode),
        "pre_op" | "preop" | "pre_operational" => Some(NmtCommand::EnterPreOperational),
        "reset_node" | "reset" => Some(NmtCommand::ResetNode),
        "reset_comm" | "reset_communication" => Some(NmtCommand::ResetCommunication),
        _ => None,
    }
}
