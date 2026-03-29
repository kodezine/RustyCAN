//! egui + eframe front-end for RustyCAN.
//!
//! # Screens
//!
//! ## Connect
//! Configures the adapter port, baud rate, per-node EDS files, and the JSONL
//! log path before starting a session. Key behaviours:
//!
//! - **Dongle polling** — a one-shot background thread probes the adapter every
//!   [`PROBE_INTERVAL_SECS`] seconds. The Connect button is disabled (with a
//!   tooltip) until the probe returns `true`.
//! - **Node ID from EDS** — when the user browses to an EDS file, the Node ID
//!   box is pre-filled from `[DeviceComissioning] NodeId` if that key exists.
//!   Accepted formats: decimal (`5`), `0x`-prefix hex (`0x05`), `H`/`h`-suffix
//!   hex (`05H`). The box remains freely editable.
//! - **EDS optional** — leaving the EDS path blank is allowed; the node will
//!   appear in the NMT table and any PDO frames will show as raw Byte0…ByteN.
//! - **Zero nodes** is valid — the monitor captures all heartbeats on the bus.
//!
//! ## Monitor
//! Three collapsible panels updated every frame by draining the event channel:
//!
//! - **NMT Status** — node state table (colour-coded) with a broadcast command
//!   strip and per-row action buttons (Start / Stop / Pre-Op / Reset / Reset Comm).
//!   Sends frames via [`CanCommand::SendNmt`]; state updates only on incoming
//!   heartbeats (no optimistic UI changes).
//! - **PDO Live Values** — last decoded value + age for every signal from every
//!   TPDO/RPDO frame seen since connect.
//! - **SDO Log** — scrollable ring-buffer (last 50 entries, stick-to-bottom).
//!
//! # Architecture
//!
//! `AppState` and `CanEvent` live in `crate::app` and are UI-independent.
//! The session lifecycle (EDS loading, adapter open, recv thread) lives in
//! `crate::session`. The GUI and the recv thread communicate through two
//! `mpsc` channels:
//!
//! ```text
//! recv thread  ──(CanEvent)──►  GUI (render_monitor → apply_event)
//! GUI          ──(CanCommand)─►  recv thread (drain at top of recv loop)
//! ```
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use chrono::Local;
use eframe::egui;
use rfd::FileDialog;

use crate::app::{apply_event, AppState, CanEvent};
use crate::canopen::nmt::{NmtCommand, NmtState};
use crate::canopen::sdo::SdoDirection;
use crate::eds;
use crate::session::{self, CanCommand, SessionConfig};

// ─── Top-level app ────────────────────────────────────────────────────────────

pub struct RustyCanApp {
    screen: Screen,
}

enum Screen {
    Connect(ConnectForm),
    Monitor(Box<MonitorView>),
}

impl RustyCanApp {
    fn new() -> Self {
        RustyCanApp {
            screen: Screen::Connect(ConnectForm::default()),
        }
    }
}

impl eframe::App for RustyCanApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint();

        let mut next_screen: Option<Screen> = None;

        match &mut self.screen {
            Screen::Connect(form) => {
                if let Some(s) = render_connect(ctx, form) {
                    next_screen = Some(s);
                }
            }
            Screen::Monitor(view) => {
                if let Some(s) = render_monitor(ctx, view.as_mut()) {
                    next_screen = Some(s);
                }
            }
        }

        if let Some(s) = next_screen {
            self.screen = s;
        }
    }
}

// ─── Connect screen ───────────────────────────────────────────────────────────

struct ConnectForm {
    port: String,
    baud: String,
    nodes: Vec<NodeEntry>,
    log_path: String,
    /// Non-None when the last connect attempt failed.
    error: Option<String>,
    /// Whether the dongle probe reported success.
    dongle_connected: bool,
    /// One-shot channel receiving the result of the latest probe.
    probe_rx: Option<mpsc::Receiver<bool>>,
    /// When the most recent probe was launched.
    last_probe: Option<Instant>,
    /// If true, no CAN frames are transmitted (commands are silently dropped).
    listen_only: bool,
}

impl Clone for ConnectForm {
    fn clone(&self) -> Self {
        ConnectForm {
            port: self.port.clone(),
            baud: self.baud.clone(),
            nodes: self.nodes.clone(),
            log_path: self.log_path.clone(),
            error: self.error.clone(),
            dongle_connected: self.dongle_connected,
            // Probe state is not preserved across clones; the new form will
            // start its own probe cycle on the next render frame.
            probe_rx: None,
            last_probe: None,
            listen_only: self.listen_only,
        }
    }
}

#[derive(Clone, Default)]
struct NodeEntry {
    id_str: String,
    eds_path: String,
}

impl Default for ConnectForm {
    fn default() -> Self {
        let ts = Local::now().format("%Y-%d-%m-%H-%M-%S");
        ConnectForm {
            port: "1".into(),
            baud: "250000".into(),
            nodes: vec![NodeEntry::default()],
            log_path: format!("rustycan-{ts}.jsonl"),
            error: None,
            dongle_connected: false,
            probe_rx: None,
            last_probe: None,
            listen_only: false,
        }
    }
}

impl ConnectForm {
    /// Validate the form and start a session.
    fn try_connect(&self) -> Result<MonitorView, String> {
        let baud: u32 = self
            .baud
            .trim()
            .parse()
            .map_err(|_| format!("Invalid baud rate: {:?}", self.baud))?;

        let nodes: Vec<(u8, Option<PathBuf>)> = self
            .nodes
            .iter()
            .map(|e| {
                let id: u8 = eds::parse_node_id_str(e.id_str.trim()).ok_or_else(|| {
                    format!(
                        "Invalid node ID: {:?} (expected 1–127, decimal or 0x/H hex)",
                        e.id_str
                    )
                })?;
                let path = if e.eds_path.trim().is_empty() {
                    None
                } else {
                    Some(PathBuf::from(e.eds_path.trim()))
                };
                Ok((id, path))
            })
            .collect::<Result<_, String>>()?;

        let config = SessionConfig {
            port: self.port.trim().to_string(),
            baud,
            nodes,
            log_path: self.log_path.trim().to_string(),
            listen_only: self.listen_only,
        };

        let (rx, cmd_tx, node_labels) = session::start(config)?;

        let mut state = AppState::new(self.log_path.trim().to_string());
        state.init_nodes(&node_labels);

        // Save a clean copy of the form (no error) to restore on disconnect.
        let mut saved_form = self.clone();
        saved_form.error = None;

        Ok(MonitorView {
            rx,
            cmd_tx,
            state,
            form: saved_form,
            disconnected: false,
            listen_only: self.listen_only,
        })
    }
}

const BAUD_OPTIONS: &[&str] = &["125000", "250000", "500000", "1000000"];
/// How often to re-probe the dongle (seconds).
const PROBE_INTERVAL_SECS: u64 = 2;

fn render_connect(ctx: &egui::Context, form: &mut ConnectForm) -> Option<Screen> {
    // ── Dongle probe cycle ────────────────────────────────────────────────────
    // 1. Drain any pending probe result.
    if let Some(rx) = &form.probe_rx {
        if let Ok(result) = rx.try_recv() {
            form.dongle_connected = result;
            form.probe_rx = None;
        }
    }
    // 2. Launch a new one-shot probe thread if enough time has elapsed.
    let should_probe = form.probe_rx.is_none()
        && form
            .last_probe
            .map(|t| t.elapsed().as_secs() >= PROBE_INTERVAL_SECS)
            .unwrap_or(true); // never probed yet → probe immediately

    if should_probe {
        let port = form.port.trim().to_string();
        let baud: u32 = form.baud.trim().parse().unwrap_or(250_000);
        let (probe_tx, probe_rx) = mpsc::channel::<bool>();
        std::thread::spawn(move || {
            let _ = probe_tx.send(session::probe_adapter(&port, baud));
        });
        form.probe_rx = Some(probe_rx);
        form.last_probe = Some(Instant::now());
    }
    let mut transition = None;

    egui::CentralPanel::default().show(ctx, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(20.0);
                ui.heading(format!("RustyCAN  {}", env!("RUSTYCAN_VERSION")));
                ui.add_space(20.0);
            });

            // ── Connection settings ───────────────────────────────────────
            egui::CollapsingHeader::new(egui::RichText::new("Connection").strong())
                .default_open(true)
                .show(ui, |ui| {
                    egui::Grid::new("conn_grid")
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Port:");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut form.port)
                                        .desired_width(80.0)
                                        .hint_text("1"),
                                );
                                // Dongle status indicator
                                if form.dongle_connected {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(0, 200, 80),
                                        "\u{25cf} Dongle: Connected",
                                    );
                                } else {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(200, 60, 60),
                                        "\u{25cf} Dongle: Not detected",
                                    );
                                }
                            });
                            ui.end_row();

                            ui.label("Baud rate (bps):");
                            egui::ComboBox::from_id_salt("baud_combo")
                                .selected_text(&form.baud)
                                .show_ui(ui, |ui| {
                                    for &b in BAUD_OPTIONS {
                                        ui.selectable_value(&mut form.baud, b.to_string(), b);
                                    }
                                });
                            ui.end_row();

                            ui.label("Log file:");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut form.log_path)
                                        .desired_width(240.0)
                                        .hint_text("rustycan.jsonl"),
                                );
                                if ui.small_button("Browse…").clicked() {
                                    if let Some(path) = FileDialog::new()
                                        .add_filter("JSONL", &["jsonl", "json"])
                                        .set_title("Choose log file location")
                                        .save_file()
                                    {
                                        form.log_path = path.to_string_lossy().into_owned();
                                    }
                                }
                            });
                            ui.end_row();

                            ui.label("Mode:");
                            ui.checkbox(&mut form.listen_only, "Listen-only (passive)");
                            ui.end_row();
                        });
                });

            ui.add_space(8.0);

            // ── Node configuration ────────────────────────────────────────
            egui::CollapsingHeader::new(egui::RichText::new("Nodes").strong())
                .default_open(true)
                .show(ui, |ui| {
                    let mut to_remove: Option<usize> = None;

                    egui::Grid::new("nodes_grid")
                        .num_columns(4)
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                            ui.strong("Node ID");
                            ui.strong("EDS file path");
                            ui.label(""); // browse-button column header
                            ui.label(""); // remove-button column header
                            ui.end_row();

                            for (i, entry) in form.nodes.iter_mut().enumerate() {
                                ui.add(
                                    egui::TextEdit::singleline(&mut entry.id_str)
                                        .desired_width(60.0)
                                        .hint_text("e.g. 1 or 0x01"),
                                );
                                ui.add(
                                    egui::TextEdit::singleline(&mut entry.eds_path)
                                        .desired_width(380.0)
                                        .hint_text("/path/to/device.eds"),
                                );
                                if ui.small_button("Browse…").clicked() {
                                    if let Some(path) = FileDialog::new()
                                        .add_filter("EDS", &["eds", "EDS"])
                                        .set_title("Select EDS file")
                                        .pick_file()
                                    {
                                        // Auto-populate node ID from [DeviceComissioning] NodeId if present.
                                        if let Some(id) = eds::parse_node_id(&path) {
                                            entry.id_str = id.to_string();
                                        }
                                        entry.eds_path = path.to_string_lossy().into_owned();
                                    }
                                }
                                if ui.small_button("✕").clicked() {
                                    to_remove = Some(i);
                                }
                                ui.end_row();
                            }
                        });

                    if let Some(i) = to_remove {
                        form.nodes.remove(i);
                    }

                    if ui.button("+ Add node").clicked() {
                        form.nodes.push(NodeEntry::default());
                    }
                });

            ui.add_space(16.0);

            // ── Error label + Connect button ──────────────────────────────
            if let Some(err) = &form.error {
                ui.colored_label(egui::Color32::RED, err);
                ui.add_space(6.0);
            }

            let connect_btn = egui::Button::new("  Connect  ");
            let resp = ui.add_enabled(form.dongle_connected, connect_btn);
            if !form.dongle_connected {
                resp.on_disabled_hover_text("Connect a CAN dongle first");
            } else if resp.clicked() {
                match form.try_connect() {
                    Ok(view) => transition = Some(Screen::Monitor(Box::new(view))),
                    Err(e) => form.error = Some(e),
                }
            }

            ui.add_space(20.0);
        });
    });

    transition
}

// ─── Monitor view ─────────────────────────────────────────────────────────────

struct MonitorView {
    rx: mpsc::Receiver<CanEvent>,
    cmd_tx: mpsc::Sender<CanCommand>,
    state: AppState,
    /// Saved form — restored when the user clicks Disconnect.
    form: ConnectForm,
    disconnected: bool,
    listen_only: bool,
}

fn render_monitor(ctx: &egui::Context, view: &mut MonitorView) -> Option<Screen> {
    // Drain all pending CAN events; intercept AdapterError before rendering.
    let mut adapter_error: Option<String> = None;
    loop {
        match view.rx.try_recv() {
            Ok(CanEvent::AdapterError(e)) => {
                adapter_error = Some(e);
                break;
            }
            Ok(ev) => apply_event(&mut view.state, ev),
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => {
                view.disconnected = true;
                break;
            }
        }
    }

    let mut disconnect_clicked = false;

    // ── Top toolbar ───────────────────────────────────────────────────────
    egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.strong("RustyCAN");
            ui.separator();
            ui.label(format!(
                "Port {}  ·  {} bps  ·  {} node(s)",
                view.form.port,
                view.form.baud,
                view.form.nodes.len(),
            ));
            if view.listen_only {
                ui.separator();
                ui.colored_label(egui::Color32::YELLOW, "LISTEN-ONLY");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Disconnect").clicked() {
                    disconnect_clicked = true;
                }
            });
        });
    });

    // ── Bottom status bar ─────────────────────────────────────────────────
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(format!(
                "Frames/s: {:.1}   Total: {}   Log: {}",
                view.state.fps, view.state.total_frames, view.state.log_path,
            ));
            if view.disconnected {
                ui.separator();
                ui.colored_label(egui::Color32::RED, "⚠ Adapter disconnected");
            }
        });
    });

    // ── Central panel ─────────────────────────────────────────────────────
    egui::CentralPanel::default().show(ctx, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            nmt_section(ui, &view.state, &view.cmd_tx, view.listen_only);
            ui.add_space(4.0);
            pdo_section(ui, &view.state);
            ui.add_space(4.0);
            sdo_section(ui, &view.state);
        });
    });

    // Decide on screen transition AFTER all panels have been rendered this frame.
    if let Some(e) = adapter_error {
        let mut form = std::mem::take(&mut view.form);
        form.error = Some(e);
        return Some(Screen::Connect(form));
    }

    if disconnect_clicked {
        let mut form = std::mem::take(&mut view.form);
        form.error = None;
        return Some(Screen::Connect(form));
    }

    None
}

// ─── NMT Status section ───────────────────────────────────────────────────────

/// The five NMT master commands available for user action.
fn nmt_actions() -> [(&'static str, NmtCommand); 5] {
    [
        ("Start", NmtCommand::StartRemoteNode),
        ("Stop", NmtCommand::StopRemoteNode),
        ("Pre-Op", NmtCommand::EnterPreOperational),
        ("Reset", NmtCommand::ResetNode),
        ("Reset Comm", NmtCommand::ResetCommunication),
    ]
}

fn nmt_section(
    ui: &mut egui::Ui,
    state: &AppState,
    cmd_tx: &mpsc::Sender<CanCommand>,
    listen_only: bool,
) {
    egui::CollapsingHeader::new(egui::RichText::new("NMT Status").strong())
        .default_open(true)
        .show(ui, |ui| {
            // ── Broadcast strip ───────────────────────────────────────────
            if !listen_only {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Broadcast:").strong());
                    for (label, cmd) in &nmt_actions() {
                        if ui.small_button(*label).clicked() {
                            let _ = cmd_tx.send(CanCommand::SendNmt {
                                command: cmd.clone(),
                                target_node: 0x00,
                            });
                        }
                    }
                });
                ui.add_space(4.0);
            }

            egui::Grid::new("nmt_grid")
                .striped(true)
                .min_col_width(80.0)
                .show(ui, |ui| {
                    ui.strong("Node");
                    ui.strong("EDS");
                    ui.strong("State");
                    ui.strong("Last seen");
                    if !listen_only {
                        ui.strong("Actions");
                    }
                    ui.end_row();

                    let mut ids: Vec<u8> = state.node_map.keys().copied().collect();
                    ids.sort();

                    for id in ids {
                        let (eds_name, nmt_state, last_seen) = &state.node_map[&id];
                        let age = last_seen
                            .map(|t| {
                                let s = t.elapsed().as_secs_f64();
                                if s < 60.0 {
                                    format!("{s:.1}s ago")
                                } else {
                                    format!("{:.0}m ago", s / 60.0)
                                }
                            })
                            .unwrap_or_else(|| "—".into());

                        let (label, color) = nmt_color(nmt_state);
                        ui.label(format!("{id}"));
                        ui.label(eds_name.as_str());
                        ui.colored_label(color, label);
                        ui.label(age);
                        // Per-node action buttons
                        if !listen_only {
                            ui.horizontal(|ui| {
                                for (btn_label, cmd) in &nmt_actions() {
                                    if ui.small_button(*btn_label).clicked() {
                                        let _ = cmd_tx.send(CanCommand::SendNmt {
                                            command: cmd.clone(),
                                            target_node: id,
                                        });
                                    }
                                }
                            });
                        }
                        ui.end_row();
                    }

                    if state.node_map.is_empty() {
                        ui.label("No nodes detected yet.");
                        ui.end_row();
                    }
                });
        });
}

fn nmt_color(state: &NmtState) -> (&'static str, egui::Color32) {
    match state {
        NmtState::Operational => ("OPERATIONAL", egui::Color32::from_rgb(0, 200, 80)),
        NmtState::PreOperational => ("PRE-OPERATIONAL", egui::Color32::YELLOW),
        NmtState::Stopped => ("STOPPED", egui::Color32::RED),
        NmtState::Bootup => ("BOOTUP", egui::Color32::from_rgb(80, 160, 255)),
        NmtState::Unknown(_) => ("UNKNOWN", egui::Color32::DARK_GRAY),
    }
}

// ─── PDO Live Values section ─────────────────────────────────────────────────

fn pdo_section(ui: &mut egui::Ui, state: &AppState) {
    egui::CollapsingHeader::new(egui::RichText::new("PDO Live Values").strong())
        .default_open(true)
        .show(ui, |ui| {
            egui::Grid::new("pdo_grid")
                .striped(true)
                .min_col_width(80.0)
                .show(ui, |ui| {
                    ui.strong("Node");
                    ui.strong("PDO");
                    ui.strong("Signal");
                    ui.strong("Value");
                    ui.strong("Updated");
                    ui.end_row();

                    let mut keys: Vec<(u8, u8)> = state.pdo_values.keys().copied().collect();
                    keys.sort();

                    for (node_id, pdo_num) in keys {
                        if let Some((values, updated)) = state.pdo_values.get(&(node_id, pdo_num)) {
                            let age_secs = updated.elapsed().as_secs_f64();
                            let age_str = if age_secs < 60.0 {
                                format!("{age_secs:.2}s ago")
                            } else {
                                format!("{:.0}m ago", age_secs / 60.0)
                            };

                            for (i, v) in values.iter().enumerate() {
                                if i == 0 {
                                    ui.label(format!("{node_id}"));
                                    ui.label(format!("{pdo_num}"));
                                } else {
                                    ui.label("");
                                    ui.label("");
                                }
                                ui.label(v.signal_name.as_str());
                                ui.label(v.value.to_string());
                                if i == 0 {
                                    ui.label(age_str.as_str());
                                } else {
                                    ui.label("");
                                }
                                ui.end_row();
                            }
                        }
                    }

                    if state.pdo_values.is_empty() {
                        ui.label("No PDO frames received yet.");
                        ui.end_row();
                    }
                });
        });
}

// ─── SDO Log section ──────────────────────────────────────────────────────────

fn sdo_section(ui: &mut egui::Ui, state: &AppState) {
    let title = format!("SDO Log  (last {})", state.sdo_log.len());
    egui::CollapsingHeader::new(egui::RichText::new(title).strong())
        .default_open(true)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("sdo_scroll")
                .max_height(300.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for entry in &state.sdo_log {
                        let ts = entry.ts.format("%H:%M:%S%.3f").to_string();
                        let (dir_str, dir_color) = match entry.direction {
                            SdoDirection::Read => ("READ ", egui::Color32::from_rgb(80, 200, 255)),
                            SdoDirection::Write => {
                                ("WRITE", egui::Color32::from_rgb(220, 100, 220))
                            }
                        };

                        ui.horizontal(|ui| {
                            ui.monospace(format!("[{ts}]"));
                            ui.strong(format!("N{:02}", entry.node_id));
                            ui.colored_label(dir_color, dir_str);
                            ui.monospace(format!("{:04X}h/{:02X}", entry.index, entry.subindex));
                            ui.label(entry.name.as_str());
                            if let Some(v) = &entry.value {
                                ui.label(format!("= {v}"));
                            }
                            if let Some(abort) = entry.abort_code {
                                ui.colored_label(
                                    egui::Color32::RED,
                                    format!("[ABORT 0x{abort:08X}]"),
                                );
                            }
                        });
                    }

                    if state.sdo_log.is_empty() {
                        ui.label("No SDO events yet.");
                    }
                });
        });
}

// ─── Public entry point ───────────────────────────────────────────────────────

/// Launch the native egui window. Blocks until the user closes it.
/// No CLI arguments needed — all config is entered via the Connect screen.
pub fn run() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("RustyCAN  {}", env!("RUSTYCAN_VERSION")))
            .with_inner_size([1000.0, 700.0]),
        ..Default::default()
    };

    eframe::run_native(
        "RustyCAN",
        options,
        Box::new(|_cc| Ok(Box::new(RustyCanApp::new()))),
    )
}
