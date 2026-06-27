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
//! **SDO read:** `<node> <index_hex> <sub|a>`
//! - Use `a` as the sub-index to read all sub-indices sequentially (like the GUI).
//! - Example single: `1 1000 0`
//! - Example all:    `32 29FF a`
//!
//! **SDO write:** `<node> <index_hex> <sub> <hex_value>`
//! - Example: `1 6040 0 0006`

pub mod log_stream;
pub(crate) mod widgets;

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
use crate::canopen::sdo::{SdoTransferMode, SdoValue};
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

// ─── Read-all scan state ─────────────────────────────────────────────────────

/// Tracks an in-progress "read all sub-indices" scan launched with `<node> <idx> a`.
struct ReadAllState {
    node_id: u8,
    index: u16,
    /// Next subindex to request (starts at 0; updated as responses arrive).
    next_sub: u8,
    /// Highest subindex reported by sub 0.  `None` until sub 0 has been received.
    max_sub: Option<u8>,
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
    /// Waiting for y/N confirmation before starting a DFU firmware update.
    DfuConfirm,
}

/// Reason the TUI exited.
#[derive(Debug, Clone, PartialEq)]
pub enum TuiExitReason {
    /// User quit normally (q / Ctrl-C).
    Quit,
    /// User confirmed a DFU firmware update (U then y).
    DfuUpdate,
}

/// Action chosen by the user on the app-update startup prompt.
#[derive(Debug, Clone, PartialEq)]
pub enum AppUpdateAction {
    /// User chose to update now (macOS: in-place; Windows/Linux: browser opened).
    UpdateNow,
    /// User deferred — proceed with the current session.
    Defer,
}

// ─── Entry points ─────────────────────────────────────────────────────────────

/// Show a full-screen TUI prompt for an available RustyCAN app update (SOUP025).
///
/// Called from `main` **before** the CAN session starts.  On macOS Apple
/// Silicon the user can trigger an in-place download + restart; on all other
/// platforms the release page is opened in the system browser and this function
/// returns [`AppUpdateAction::Defer`] so the session can continue normally.
///
/// Key bindings:
/// - macOS: `U` / `u` → download, apply, relaunch (process exits on success)
/// - macOS: `D` / `d` / any other key → Defer
/// - Windows / Linux: `O` / `o` → open browser; `D` / any other key → Defer
pub fn show_app_update_prompt(
    release: &crate::updater::AppUpdateRelease,
) -> io::Result<AppUpdateAction> {
    use ratatui::{
        layout::Alignment,
        style::{Color, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph},
    };

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let ver = release.version_string();
    let can_download = release.can_download();

    // Build the prompt lines once.
    let title = Line::from(vec![
        Span::raw(" Rusty"),
        Span::styled("CAN", Style::default().fg(Color::Cyan)),
        Span::raw(format!(" {} available ", ver)),
    ]);
    let (action_line, key_update) = if can_download {
        ("[U] Update now  [D] Defer", 'u')
    } else {
        ("[O] Open release page  [D] Defer", 'o')
    };

    // Download progress state (macOS only).
    let mut download_progress: Option<f32> = None;
    let mut download_msg: Option<String> = None;
    let mut progress_rx: Option<std::sync::mpsc::Receiver<crate::updater::DownloadMsg>> = None;

    let action = loop {
        terminal.draw(|f| {
            let area = f.area();
            // Centre a 60×10 box.
            let box_w = 60u16.min(area.width.saturating_sub(4));
            let box_h = 10u16;
            let x = area.width.saturating_sub(box_w) / 2;
            let y = area.height.saturating_sub(box_h) / 2;
            let popup = ratatui::layout::Rect::new(x, y, box_w, box_h);

            f.render_widget(Clear, popup);

            let mut lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  A new version of RustyCAN is available: {ver}"),
                    Style::default().fg(Color::Cyan),
                )),
                Line::from(""),
            ];

            if let Some(ref msg) = download_msg {
                lines.push(Line::from(Span::styled(
                    format!("  {msg}"),
                    Style::default().fg(Color::Green),
                )));
            } else if let Some(p) = download_progress {
                let filled = ((p * 30.0) as usize).min(30);
                let bar = format!(
                    "  [{}>{}] {:.0}%",
                    "=".repeat(filled),
                    " ".repeat(30 - filled),
                    p * 100.0
                );
                lines.push(Line::from(Span::styled(
                    bar,
                    Style::default().fg(Color::Yellow),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    format!("  {action_line}"),
                    Style::default().fg(Color::White),
                )));
            }

            let block = Block::default()
                .title(title.clone())
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Blue));

            let para = Paragraph::new(lines).block(block);
            f.render_widget(para, popup);
        })?;

        // Drain download progress if active.
        let mut clear_rx = false;
        if let Some(ref rx) = progress_rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    crate::updater::DownloadMsg::Progress(p) => {
                        download_progress = Some(p);
                    }
                    crate::updater::DownloadMsg::Done(path) => {
                        download_msg = Some("Applying update, restarting\u{2026}".to_string());
                        terminal.draw(|f| {
                            // Trigger one last render to show "Applying…" before apply blocks.
                            let _ = f.area();
                        })?;
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        {
                            if let Err(e) = crate::updater::apply_and_restart(&path) {
                                download_msg =
                                    Some(format!("Update failed: {e}. Press any key to continue."));
                                clear_rx = true;
                            }
                        }
                        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                        let _ = path;
                    }
                    crate::updater::DownloadMsg::Err(e) => {
                        download_msg =
                            Some(format!("Download failed: {e}. Press any key to continue."));
                        clear_rx = true;
                    }
                }
            }
        }
        if clear_rx {
            progress_rx = None;
        }

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                // If we're mid-download, only Escape / Ctrl-C aborts (Defer).
                if progress_rx.is_some() || download_msg.is_some() {
                    // After a failure message, any key returns Defer.
                    if download_msg.is_some() {
                        break AppUpdateAction::Defer;
                    }
                    // During download, ignore keys.
                    continue;
                }

                match key.code {
                    KeyCode::Char(c) if c == key_update || c == key_update.to_ascii_uppercase() => {
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        {
                            // macOS: start background download.
                            let (tx, rx) =
                                std::sync::mpsc::channel::<crate::updater::DownloadMsg>();
                            let rel = release.clone();
                            std::thread::spawn(move || {
                                let tx_p = tx.clone();
                                match crate::updater::download_update(&rel, move |p| {
                                    let _ =
                                        tx_p.send(crate::updater::DownloadMsg::Progress(p as f32));
                                }) {
                                    Ok(path) => {
                                        let _ = tx.send(crate::updater::DownloadMsg::Done(path));
                                    }
                                    Err(e) => {
                                        let _ = tx.send(crate::updater::DownloadMsg::Err(e));
                                    }
                                }
                            });
                            progress_rx = Some(rx);
                        }
                        // Windows / Linux: open browser and defer.
                        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                        {
                            let _ = can_download; // always false on non-macOS
                            crate::updater::open_release_page(release);
                            break AppUpdateAction::Defer;
                        }
                    }
                    KeyCode::Char('d') | KeyCode::Char('D') | KeyCode::Esc => {
                        break AppUpdateAction::Defer;
                    }
                    KeyCode::Char('c')
                        if key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL) =>
                    {
                        break AppUpdateAction::Defer;
                    }
                    _ => {}
                }
            }
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(action)
}

/// Load a config file and run the full-screen TUI.
///
/// `dfu_path` — when `Some`, a `[U] update firmware` hint is shown in the
/// command bar; pressing `U` prompts y/N, and `y` exits with
/// [`TuiExitReason::DfuUpdate`] so the caller can run the DFU update.
///
/// Returns [`TuiExitReason::Quit`] on normal exit.
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
pub fn run_from_config(
    config_path: &Path,
    _http_port: u16,
    dfu_path: Option<&std::path::Path>,
) -> io::Result<TuiExitReason> {
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
    let (rx, cmd_tx, node_labels, log_path, _startup_notice) =
        crate::session::start(session_cfg)
            .map_err(|e| io::Error::other(format!("Session start failed: {e}")))?;

    let state = AppState::new(log_path, baud);
    run(rx, cmd_tx, state, node_labels, dfu_path)
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
/// - `dfu_path` — optional signed firmware path for in-TUI DFU update flow.
pub fn run(
    rx: mpsc::Receiver<CanEvent>,
    cmd_tx: mpsc::Sender<CanCommand>,
    state: AppState,
    nodes: Vec<(u8, String)>,
    dfu_path: Option<&std::path::Path>,
) -> io::Result<TuiExitReason> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, rx, cmd_tx, state, nodes, dfu_path);

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
    dfu_path: Option<&std::path::Path>,
) -> io::Result<TuiExitReason> {
    state.init_nodes(&nodes);

    let mut mode = TuiMode::Normal;
    let mut log_entries: VecDeque<String> = VecDeque::with_capacity(512);
    let mut show_log = true;
    let mut read_all_state: Option<ReadAllState> = None;
    let tick = Duration::from_millis(100);

    loop {
        // Drain all pending decoded CAN events (non-blocking); collect log lines.
        let alive = drain_events_with_log(
            &mut state,
            &rx,
            &mut log_entries,
            &mut read_all_state,
            &cmd_tx,
        );

        // Render current frame.
        draw(terminal, &state, &mode, &log_entries, show_log, dfu_path)?;

        if !alive {
            // Adapter thread died — show the final frame for a moment then exit.
            std::thread::sleep(Duration::from_millis(500));
            return Ok(TuiExitReason::Quit);
        }

        // Poll keyboard for up to `tick` duration.
        if event::poll(tick)? {
            if let Event::Key(key) = event::read()? {
                match &mut mode {
                    TuiMode::Normal => match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') => {
                            return Ok(TuiExitReason::Quit);
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(TuiExitReason::Quit);
                        }
                        // DFU update: only available when a signed binary path was supplied.
                        KeyCode::Char('U') | KeyCode::Char('u') if dfu_path.is_some() => {
                            mode = TuiMode::DfuConfirm;
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
                    TuiMode::DfuConfirm => match key.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            return Ok(TuiExitReason::DfuUpdate);
                        }
                        KeyCode::Char('n')
                        | KeyCode::Char('N')
                        | KeyCode::Esc
                        | KeyCode::Char('q') => {
                            mode = TuiMode::Normal;
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
                            // Special case: SDO read-all when sub-index is "a".
                            let handled = if kind_clone == InputKind::SdoRead {
                                let parts: Vec<&str> = input.split_whitespace().collect();
                                if parts.len() == 3 && matches!(parts[2], "a" | "A") {
                                    let parsed = parts[0].parse::<u8>().ok().zip(
                                        u16::from_str_radix(parts[1].trim_start_matches("0x"), 16)
                                            .ok(),
                                    );
                                    if let Some((node_id, index)) = parsed {
                                        read_all_state = Some(ReadAllState {
                                            node_id,
                                            index,
                                            next_sub: 0,
                                            max_sub: None,
                                        });
                                        let _ = cmd_tx.send(CanCommand::SdoRead {
                                            node_id,
                                            index,
                                            subindex: 0,
                                            mode: SdoTransferMode::Auto,
                                        });
                                        true
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                }
                            } else {
                                false
                            };
                            if !handled {
                                if let Some(cmd) = parse_command(&kind_clone, &input) {
                                    // Non-blocking send; ignore if the session thread is gone.
                                    let _ = cmd_tx.send(cmd);
                                }
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
    dfu_path: Option<&std::path::Path>,
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
        let bundled_ver = crate::bundled_firmware_version();
        widgets::render_command_bar(
            f,
            mode,
            dfu_path,
            state.device_fw_version,
            bundled_ver,
            vertical[idx],
        );
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
    read_all: &mut Option<ReadAllState>,
    cmd_tx: &mpsc::Sender<CanCommand>,
) -> bool {
    use chrono::Utc;
    const LOG_CAP: usize = 512;

    loop {
        match rx.try_recv() {
            Ok(event) => {
                // Advance read-all scan when the expected SDO result arrives.
                if let CanEvent::Sdo(ref entry) = event {
                    if let Some(ref mut ra) = *read_all {
                        if entry.node_id == ra.node_id
                            && entry.index == ra.index
                            && entry.subindex == ra.next_sub
                        {
                            if ra.max_sub.is_none() {
                                // Sub 0: read the highest sub-index count.
                                let max_sub = match entry.value.as_ref() {
                                    Some(SdoValue::U8(n)) => *n,
                                    Some(SdoValue::Bytes(b)) => b.first().copied().unwrap_or(0),
                                    _ => 0,
                                };
                                if max_sub >= 1 {
                                    ra.max_sub = Some(max_sub);
                                    ra.next_sub = 1;
                                    let _ = cmd_tx.send(CanCommand::SdoRead {
                                        node_id: ra.node_id,
                                        index: ra.index,
                                        subindex: 1,
                                        mode: SdoTransferMode::Auto,
                                    });
                                } else {
                                    *read_all = None;
                                }
                            } else {
                                // Sub N: fire N+1 if still within range.
                                // Compare before incrementing so sub-index 255
                                // (a valid u8) cannot overflow `next_sub + 1`.
                                let max_sub = ra.max_sub.unwrap_or(0);
                                if ra.next_sub < max_sub {
                                    let next = ra.next_sub + 1;
                                    ra.next_sub = next;
                                    let _ = cmd_tx.send(CanCommand::SdoRead {
                                        node_id: ra.node_id,
                                        index: ra.index,
                                        subindex: next,
                                        mode: SdoTransferMode::Auto,
                                    });
                                } else {
                                    *read_all = None;
                                }
                            }
                        }
                    }
                }

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
                .or_else(|| {
                    entry
                        .abort_code
                        .map(|c| format!("ABORT 0x{c:08X}: {}", sdo_abort_description(c)))
                })
                .unwrap_or_else(|| "pending".into());
            let fallback_name = format!("{:04X}h/{:02X}", entry.index, entry.subindex);
            let name_part = if entry.name == fallback_name {
                String::new()
            } else {
                format!("  {}", entry.name)
            };
            Some(format!(
                "SDO  node {}  {dir}  {:04X}:{:02X}{name_part} = {}",
                entry.node_id, entry.index, entry.subindex, val
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
        CanEvent::RawFrame { cob_id, data, port } => {
            let hex: String = data
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ");
            let ch = if *port == 0 { "FDCAN1" } else { "FDCAN2" };
            Some(format!(
                "RAW  [{cob_id:#05X}]  {ch}  dlc={}  {hex}",
                data.len()
            ))
        }
        CanEvent::AdapterDisconnected => {
            Some("ADAPTER DISCONNECTED — waiting to reconnect…".into())
        }
        CanEvent::AdapterReconnected => Some("ADAPTER RECONNECTED — session resumed".into()),
        CanEvent::FirmwareVersion(maj, min, pat) => Some(format!("FW VERSION  v{maj}.{min}.{pat}")),
    }
}

// ─── Command parsing ──────────────────────────────────────────────────────────

/// Parse a typed command string and return the corresponding [`CanCommand`].
///
/// Returns `None` on parse error (the TUI just silently discards invalid input;
/// a future improvement could show an inline error in the command bar).
pub(crate) fn parse_command(kind: &InputKind, input: &str) -> Option<CanCommand> {
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

/// Return a short English description for a CANopen SDO abort code (CiA 301).
pub(crate) fn sdo_abort_description(code: u32) -> &'static str {
    match code {
        0x05030000 => "toggle bit not alternated",
        0x05040000 => "SDO protocol timed out",
        0x05040001 => "command specifier not valid",
        0x05040002 => "invalid block size",
        0x05040003 => "invalid sequence number",
        0x05040004 => "CRC error",
        0x05040005 => "out of memory",
        0x06010000 => "unsupported access to object",
        0x06010001 => "attempt to read a write-only object",
        0x06010002 => "attempt to write a read-only object",
        0x06020000 => "object does not exist",
        0x06040041 => "object cannot be mapped to PDO",
        0x06040042 => "PDO mapping would exceed PDO length",
        0x06040043 => "general parameter incompatibility",
        0x06040047 => "general internal incompatibility in device",
        0x06060000 => "access failed due to hardware error",
        0x06070010 => "data type / length mismatch",
        0x06070012 => "data type mismatch: length too high",
        0x06070013 => "data type mismatch: length too low",
        0x06090011 => "sub-index does not exist",
        0x06090030 => "invalid value for parameter",
        0x06090031 => "value too high",
        0x06090032 => "value too low",
        0x06090036 => "maximum value less than minimum",
        0x060A0023 => "resource not available",
        0x08000000 => "general error",
        0x08000020 => "data cannot be transferred to application",
        0x08000021 => "data cannot be transferred: local control",
        0x08000022 => "data cannot be transferred: device state",
        0x08000023 => "object dictionary generation failed",
        0x08000024 => "no data available",
        _ => "unknown abort code",
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

// ─── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canopen::nmt::NmtCommand;
    use crate::canopen::sdo::SdoTransferMode;
    use crate::session::CanCommand;

    // ── NMT command parser ────────────────────────────────────────────────────

    #[test]
    fn nmt_parse_start() {
        assert!(matches!(
            parse_command(&InputKind::Nmt, "1 start"),
            Some(CanCommand::SendNmt {
                command: NmtCommand::StartRemoteNode,
                target_node: 1,
            })
        ));
    }

    #[test]
    fn nmt_parse_broadcast_preop() {
        // node_id 0 broadcasts to all nodes
        assert!(matches!(
            parse_command(&InputKind::Nmt, "0 pre_op"),
            Some(CanCommand::SendNmt {
                command: NmtCommand::EnterPreOperational,
                target_node: 0,
            })
        ));
    }

    #[test]
    fn nmt_parse_invalid_no_panic() {
        assert!(parse_command(&InputKind::Nmt, "").is_none(), "empty → None");
        assert!(
            parse_command(&InputKind::Nmt, "1").is_none(),
            "missing command → None"
        );
        assert!(
            parse_command(&InputKind::Nmt, "1 bogus").is_none(),
            "unknown command → None"
        );
        assert!(
            parse_command(&InputKind::SdoRead, "1 gg 0").is_none(),
            "bad index → None"
        );
    }

    // ── SDO command parser ────────────────────────────────────────────────────

    #[test]
    fn sdo_read_parse() {
        assert!(matches!(
            parse_command(&InputKind::SdoRead, "1 1000 0"),
            Some(CanCommand::SdoRead {
                node_id: 1,
                index: 0x1000,
                subindex: 0,
                mode: SdoTransferMode::Auto,
            })
        ));
    }

    #[test]
    fn sdo_write_parse() {
        // "0006" → 4 hex chars → 2 bytes LE: value 6 = [0x06, 0x00]
        match parse_command(&InputKind::SdoWrite, "1 6040 0 0006") {
            Some(CanCommand::SdoWrite {
                node_id: 1,
                index: 0x6040,
                subindex: 0,
                data,
                mode: SdoTransferMode::Auto,
            }) => {
                assert_eq!(data, vec![0x06, 0x00]);
            }
            _ => panic!("expected SdoWrite for '1 6040 0 0006'"),
        }
    }
}
