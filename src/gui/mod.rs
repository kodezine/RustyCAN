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
//! - **Listen-only mode** — checkbox that sets [`SessionConfig::listen_only`];
//!   all [`CanCommand`] variants are silently dropped in the recv thread so
//!   no frames are ever transmitted.
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
//! ## Status bar (bottom)
//! Three items separated by dividers, rendered every frame:
//!
//! - **fps / total** — rolling frames-per-second (2 s window) and cumulative
//!   frame count since connect.
//! - **Bus load bar** — 20-block `█`/`░` bar built with [`egui::text::LayoutJob`]
//!   so each colour zone is a separate text run with no gaps:
//!   - Blue  (`Color32` RGB 60/130/220) for the 0–30 % zone.
//!   - Yellow (RGB 230/170/0) for the 30–70 % zone.
//!   - Red   (RGB 220/60/60)  for the >70 % zone.
//!
//!   The percentage label after the bar matches the colour of the highest
//!   filled zone. Load is estimated as `fps × 125 bits ÷ baud_rate × 100`.
//!   See [`bus_load_bar`].
//! - **Log path** — filename only; full path shown on hover.
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
use eframe::egui::{self, vec2, Button, Color32};
use rfd::FileDialog;

use crate::app::{apply_event, AppState, CanEvent};
use crate::canopen::nmt::{NmtCommand, NmtState};
use crate::canopen::pdo::PdoRawValue;
use crate::canopen::sdo::SdoDirection;
use crate::eds;
use crate::session::{self, CanCommand, SessionConfig};

// ─── Icon glyphs (Font Awesome codepoints present in MesloLGS NF) ────────────
mod icons {
    // App / toolbar
    pub const APP: &str = "\u{f085}"; // cogs
    pub const PORT: &str = "\u{f1e6}"; // plug
    pub const BAUD: &str = "\u{f197}"; // keyboard
    pub const NODES: &str = "\u{f0c0}"; // users
    pub const FPS: &str = "\u{f080}"; // bar-chart
    pub const LOG: &str = "\u{f0f6}"; // file-text
    pub const DISCONNECT: &str = "\u{f071}"; // warning
                                             // Dongle status
    pub const PLUG_OK: &str = "\u{f1e6}"; // plug
    pub const PLUG_FAIL: &str = "\u{f1e6}"; // plug (color differentiates)
                                            // Browse / file picker
    pub const BROWSE: &str = "\u{f07c}"; // folder-open
                                         // Node management
    pub const ADD_NODE: &str = "\u{f234}"; // user-plus
    pub const REMOVE_NODE: &str = "\u{f1f8}"; // trash
                                              // Section headers
    pub const NMT_HEADER: &str = "\u{f233}"; // server
    pub const PDO_HEADER: &str = "\u{f1de}"; // sliders
    pub const SDO_HEADER: &str = "\u{f0f6}"; // file-text
                                             // NMT states
    pub const STATE_OP: &str = "\u{f058}"; // check-circle
    pub const STATE_PREOP: &str = "\u{f017}"; // clock
    pub const STATE_STOP: &str = "\u{f057}"; // times-circle
    pub const STATE_BOOT: &str = "\u{f135}"; // rocket
    pub const STATE_UNK: &str = "\u{f128}"; // question
    pub const BROADCAST: &str = "\u{f0a1}"; // bullhorn
                                            // NMT action buttons
    pub const ACT_START: &str = "\u{f04b}"; // play
    pub const ACT_STOP: &str = "\u{f04d}"; // stop
    pub const ACT_PREOP: &str = "\u{f04c}"; // pause
    pub const ACT_RESET: &str = "\u{f021}"; // refresh
    pub const ACT_RESET_COMM: &str = "\u{f0e2}"; // undo
                                                 // Validation feedback
    pub const WARN: &str = "\u{f071}"; // exclamation-triangle
    pub const ERROR: &str = "\u{f06a}"; // exclamation-circle
}

/// Fixed width for every NMT action column — sized so the widest header ("Pre-Op") fits.
const NMT_COL_W: f32 = 52.0;
/// Fixed width of the State column — wide enough for the longest badge ("PRE-OP" + icon).
const STATE_COL_W: f32 = 90.0;

/// Format a bps string (e.g. "250000") with thousands separators → "250,000".
fn format_bps(s: &str) -> String {
    match s.trim().parse::<u64>() {
        Ok(n) => {
            let raw = n.to_string();
            let mut out = String::new();
            for (i, ch) in raw.chars().rev().enumerate() {
                if i > 0 && i % 3 == 0 {
                    out.push(',');
                }
                out.push(ch);
            }
            out.chars().rev().collect()
        }
        Err(_) => s.to_string(),
    }
}

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
    /// Live validation warnings shown before the connect button.
    warnings: Vec<String>,
    /// Index of the node awaiting remove-confirmation, if any.
    confirm_remove: Option<usize>,
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
            warnings: self.warnings.clone(),
            confirm_remove: self.confirm_remove,
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
            warnings: vec![],
            confirm_remove: None,
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
        // Reject duplicate node IDs.
        let mut seen = std::collections::HashSet::new();
        for (id, _) in &nodes {
            if !seen.insert(id) {
                return Err(format!(
                    "Node ID {} is used more than once — each node must have a unique ID",
                    id
                ));
            }
        }
        let config = SessionConfig {
            port: self.port.trim().to_string(),
            baud,
            nodes,
            log_path: self.log_path.trim().to_string(),
            listen_only: self.listen_only,
        };

        let (rx, cmd_tx, node_labels) = session::start(config)?;

        let mut state = AppState::new(self.log_path.trim().to_string(), baud);
        state.init_nodes(&node_labels);

        // Save a clean copy of the form (no error) to restore on disconnect.
        let mut saved_form = self.clone();
        saved_form.error = None;

        // Build node_id → full EDS path map for hover tooltips in the monitor.
        let node_eds_paths: std::collections::HashMap<u8, String> = self
            .nodes
            .iter()
            .filter_map(|e| {
                eds::parse_node_id_str(e.id_str.trim()).map(|id| (id, e.eds_path.clone()))
            })
            .collect();

        Ok(MonitorView {
            rx,
            cmd_tx,
            state,
            form: saved_form,
            disconnected: false,
            listen_only: self.listen_only,
            node_eds_paths,
        })
    }
}

/// Standard CANopen baud rates supported by PEAK PCAN adapters, in bps.
const BAUD_OPTIONS: &[&str] = &[
    "10000", "20000", "50000", "100000", "125000", "250000", "500000", "800000", "1000000",
];
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

    // Estimated content height so we can split surplus space equally top/bottom.
    const CONNECT_CONTENT_H: f32 = 560.0;

    egui::CentralPanel::default().show(ctx, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            let top_pad = ((ui.available_height() - CONNECT_CONTENT_H) / 2.0).max(20.0);
            ui.vertical_centered(|ui| {
                ui.add_space(top_pad);
                ui.heading(egui::RichText::new(format!("{} RustyCAN", icons::APP)).strong());
                ui.label(
                    egui::RichText::new(env!("RUSTYCAN_VERSION"))
                        .size(12.0)
                        .color(egui::Color32::from_gray(140)),
                );
                ui.add_space(16.0);
            });

            // ── Connection settings ───────────────────────────────────────
            egui::Frame::group(ui.style())
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    egui::CollapsingHeader::new(
                        egui::RichText::new("Connection").size(20.0).strong(),
                    )
                    .default_open(true)
                    .show(ui, |ui| {
                        egui::Grid::new("conn_grid")
                            .num_columns(2)
                            .spacing([12.0, 10.0])
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
                                            Color32::from_rgb(0, 200, 80),
                                            format!("{} Dongle: Connected", icons::PLUG_OK),
                                        );
                                    } else {
                                        ui.colored_label(
                                            Color32::from_rgb(200, 60, 60),
                                            format!("{} Dongle: Not detected", icons::PLUG_FAIL),
                                        );
                                    }
                                });
                                ui.end_row();

                                ui.label("Baud rate (bps):");
                                egui::ComboBox::from_id_salt("baud_combo")
                                    .selected_text(format_bps(&form.baud))
                                    .show_ui(ui, |ui| {
                                        for &b in BAUD_OPTIONS {
                                            ui.selectable_value(
                                                &mut form.baud,
                                                b.to_string(),
                                                format_bps(b),
                                            );
                                        }
                                    });
                                ui.end_row();

                                ui.label("Log file:");
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        // Invisible placeholder matching the remove-node button width
                                        // so the browse button aligns with the EDS browse button above/below.
                                        ui.allocate_exact_size(
                                            vec2(36.0, 28.0),
                                            egui::Sense::hover(),
                                        );
                                        if ui
                                            .add_sized(
                                                vec2(36.0, 28.0),
                                                Button::new(
                                                    egui::RichText::new(icons::BROWSE).size(16.0),
                                                )
                                                .fill(Color32::from_rgb(170, 130, 0)),
                                            )
                                            .on_hover_text("Browse for log file…")
                                            .clicked()
                                        {
                                            if let Some(path) = FileDialog::new()
                                                .add_filter("JSONL", &["jsonl", "json"])
                                                .set_title("Choose log file location")
                                                .save_file()
                                            {
                                                form.log_path = path.to_string_lossy().into_owned();
                                            }
                                        }
                                        let log_path_id = egui::Id::new("log_path_field");
                                        let log_focused =
                                            ui.ctx().memory(|m| m.has_focus(log_path_id));
                                        // When idle show just the filename; when focused show the full path for editing.
                                        let mut log_display = if log_focused {
                                            form.log_path.clone()
                                        } else {
                                            std::path::Path::new(&form.log_path)
                                                .file_name()
                                                .map(|f| f.to_string_lossy().into_owned())
                                                .unwrap_or_else(|| form.log_path.clone())
                                        };
                                        let log_resp = ui.add(
                                            egui::TextEdit::singleline(&mut log_display)
                                                .id(log_path_id)
                                                .desired_width(f32::INFINITY)
                                                .hint_text("rustycan.jsonl"),
                                        );
                                        if log_focused && log_resp.changed() {
                                            form.log_path = log_display;
                                        }
                                        if !form.log_path.is_empty() {
                                            log_resp.on_hover_text(&form.log_path);
                                        }
                                    },
                                );
                                ui.end_row();

                                ui.label("Mode:");
                                ui.checkbox(&mut form.listen_only, "Listen-only (passive)");
                                ui.end_row();
                            });
                    }); // close Connection CollapsingHeader
                }); // close Connection Frame

            ui.add_space(16.0);

            // ── Node configuration ────────────────────────────────────────
            egui::Frame::group(ui.style())
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    egui::CollapsingHeader::new(egui::RichText::new("Nodes").size(20.0).strong())
                        .default_open(true)
                        .show(ui, |ui| {
                            let mut to_remove: Option<usize> = None;
                            let node_count = form.nodes.len();
                            // Snapshot confirm state before iter_mut() to avoid borrow conflict.
                            let confirming = form.confirm_remove;
                            let mut new_confirm: Option<Option<usize>> = None;

                            egui::Grid::new("nodes_grid")
                                .num_columns(2)
                                .spacing([12.0, 10.0])
                                .show(ui, |ui| {
                                    ui.label("Node ID");
                                    ui.label("EDS file path");
                                    ui.end_row();

                                    for (i, entry) in form.nodes.iter_mut().enumerate() {
                                        ui.add(
                                            egui::TextEdit::singleline(&mut entry.id_str)
                                                .desired_width(60.0)
                                                .hint_text("e.g. 1 or 0x01"),
                                        );
                                        // EDS path + browse + remove all in one expanding cell
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if confirming == Some(i) {
                                                    // ── Inline confirmation ──────────────────────────
                                                    if ui
                                                        .add_sized(
                                                            vec2(46.0, 28.0),
                                                            Button::new(
                                                                egui::RichText::new("Yes")
                                                                    .color(Color32::WHITE),
                                                            )
                                                            .fill(Color32::from_rgb(200, 50, 50)),
                                                        )
                                                        .on_hover_text("Confirm removal")
                                                        .clicked()
                                                    {
                                                        to_remove = Some(i);
                                                        new_confirm = Some(None);
                                                    }
                                                    if ui
                                                        .add_sized(
                                                            vec2(60.0, 28.0),
                                                            Button::new("Cancel"),
                                                        )
                                                        .on_hover_text("Keep this node")
                                                        .clicked()
                                                    {
                                                        new_confirm = Some(None);
                                                    }
                                                    ui.with_layout(
                                                        egui::Layout::left_to_right(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            ui.label("Remove this node?");
                                                        },
                                                    );
                                                    return; // skip browse + EDS field while confirming
                                                }

                                                // ── Normal mode ─────────────────────────────────────
                                                let can_remove = node_count > 1;
                                                if ui
                                                    .add_enabled(
                                                        can_remove,
                                                        Button::new(
                                                            egui::RichText::new(icons::REMOVE_NODE)
                                                                .size(16.0)
                                                                .color(if can_remove {
                                                                    Color32::from_rgb(220, 60, 60)
                                                                } else {
                                                                    Color32::from_gray(100)
                                                                }),
                                                        )
                                                        .min_size(vec2(36.0, 28.0)),
                                                    )
                                                    .on_hover_text(if can_remove {
                                                        "Remove this node"
                                                    } else {
                                                        "At least one node is required"
                                                    })
                                                    .clicked()
                                                {
                                                    new_confirm = Some(Some(i));
                                                }
                                                if ui
                                                    .add_sized(
                                                        vec2(36.0, 28.0),
                                                        Button::new(
                                                            egui::RichText::new(icons::BROWSE)
                                                                .size(16.0),
                                                        )
                                                        .fill(Color32::from_rgb(170, 130, 0)),
                                                    )
                                                    .on_hover_text("Browse for EDS file…")
                                                    .clicked()
                                                {
                                                    if let Some(path) = FileDialog::new()
                                                        .add_filter("EDS", &["eds", "EDS"])
                                                        .set_title("Select EDS file")
                                                        .pick_file()
                                                    {
                                                        // Auto-populate node ID from [DeviceComissioning] NodeId if present.
                                                        if let Some(id) = eds::parse_node_id(&path)
                                                        {
                                                            entry.id_str = id.to_string();
                                                        }
                                                        entry.eds_path =
                                                            path.to_string_lossy().into_owned();
                                                    }
                                                }
                                                let eds_id = egui::Id::new(("eds_path", i));
                                                let eds_focused =
                                                    ui.ctx().memory(|m| m.has_focus(eds_id));
                                                // When idle show just the filename; when focused show full path.
                                                let mut eds_display = if eds_focused {
                                                    entry.eds_path.clone()
                                                } else {
                                                    std::path::Path::new(&entry.eds_path)
                                                        .file_name()
                                                        .map(|f| f.to_string_lossy().into_owned())
                                                        .unwrap_or_else(|| entry.eds_path.clone())
                                                };
                                                let eds_resp = ui.add(
                                                    egui::TextEdit::singleline(&mut eds_display)
                                                        .id(eds_id)
                                                        .desired_width(f32::INFINITY)
                                                        .hint_text("/path/to/device.eds"),
                                                );
                                                if eds_focused && eds_resp.changed() {
                                                    entry.eds_path = eds_display;
                                                }
                                                if !entry.eds_path.is_empty() {
                                                    eds_resp.on_hover_text(&entry.eds_path);
                                                }
                                            },
                                        );
                                        ui.end_row();
                                    }
                                });

                            if let Some(v) = new_confirm {
                                form.confirm_remove = v;
                            }
                            if let Some(i) = to_remove {
                                form.nodes.remove(i);
                                // Clear any stale confirmation index after removal.
                                if form.confirm_remove.is_some() {
                                    form.confirm_remove = None;
                                }
                            }

                            if ui
                                .add_sized(
                                    vec2(36.0, 28.0),
                                    Button::new(
                                        egui::RichText::new(icons::ADD_NODE)
                                            .size(16.0)
                                            .color(Color32::from_rgb(60, 130, 220)),
                                    ),
                                )
                                .on_hover_text("Add new node")
                                .clicked()
                            {
                                form.nodes.push(NodeEntry::default());
                            }
                        }); // close Nodes CollapsingHeader
                }); // close Nodes Frame

            ui.add_space(20.0);

            // ── Live warnings (duplicate node IDs) ────────────────────────
            {
                let mut dupes: std::collections::HashMap<u8, usize> =
                    std::collections::HashMap::new();
                for entry in &form.nodes {
                    if let Some(id) = eds::parse_node_id_str(entry.id_str.trim()) {
                        *dupes.entry(id).or_insert(0) += 1;
                    }
                }
                form.warnings = dupes
                    .into_iter()
                    .filter(|(_, count)| *count > 1)
                    .map(|(id, _)| format!("Node ID {} is used more than once", id))
                    .collect();
                form.warnings.sort();
            }

            // ── Error / warning label + Connect button ─────────────────────
            // Always reserve space so the Connect button never shifts.
            ui.vertical_centered(|ui| {
                if let Some(err) = &form.error.clone() {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(icons::ERROR)
                                .size(16.0)
                                .color(Color32::from_rgb(220, 60, 60)),
                        );
                        ui.colored_label(Color32::from_rgb(220, 60, 60), err);
                    });
                } else if !form.warnings.is_empty() {
                    for w in &form.warnings.clone() {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(icons::WARN)
                                    .size(16.0)
                                    .color(Color32::from_rgb(220, 170, 0)),
                            );
                            ui.colored_label(Color32::from_rgb(220, 170, 0), w);
                        });
                    }
                } else {
                    ui.add_space(ui.text_style_height(&egui::TextStyle::Body));
                }
            });
            ui.add_space(6.0);

            ui.vertical_centered(|ui| {
                let has_dupes = !form.warnings.is_empty();
                let can_connect = form.dongle_connected && !has_dupes;
                let connect_btn = Button::new(egui::RichText::new("  Connect  ").size(18.0))
                    .min_size(vec2(180.0, 40.0));
                let resp = ui.add_enabled(can_connect, connect_btn);
                if !form.dongle_connected {
                    resp.on_disabled_hover_text("Connect a CAN dongle first");
                } else if has_dupes {
                    resp.on_disabled_hover_text("Fix duplicate node IDs before connecting");
                } else if resp.clicked() {
                    match form.try_connect() {
                        Ok(view) => transition = Some(Screen::Monitor(Box::new(view))),
                        Err(e) => form.error = Some(e),
                    }
                }
            });

            ui.add_space(top_pad.min(20.0));
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
    /// node_id → absolute EDS path (empty string when no EDS was configured).
    node_eds_paths: std::collections::HashMap<u8, String>,
}

/// Render a block-character bus-load bar into the current horizontal layout.
///
/// Visual: `Bus [██████░░░░░░░░░░░░░░] 28.4%`
/// Colours — blue ≤30 %, yellow 30–70 %, red >70 %.
fn bus_load_bar(ui: &mut egui::Ui, load: f64) {
    use egui::text::{LayoutJob, TextFormat};

    const BLOCKS: usize = 20;
    let filled = ((load / 100.0) * BLOCKS as f64).round() as usize;
    let filled = filled.min(BLOCKS);

    // Colour-zone boundaries in blocks (30 % → 6, 70 % → 14)
    let blue_end = ((30.0_f64 / 100.0) * BLOCKS as f64).round() as usize;
    let yellow_end = ((70.0_f64 / 100.0) * BLOCKS as f64).round() as usize;

    let blue_n = filled.min(blue_end);
    let yellow_n = if filled > blue_end {
        (filled - blue_end).min(yellow_end - blue_end)
    } else {
        0
    };
    let red_n = filled.saturating_sub(yellow_end);
    let empty_n = BLOCKS - filled;

    let font = egui::FontId::monospace(13.0);
    let mk = |color: Color32| TextFormat {
        color,
        font_id: font.clone(),
        ..Default::default()
    };

    let mut job = LayoutJob::default();
    job.append("Bus ", 0.0, mk(Color32::from_gray(160)));
    job.append("[", 0.0, mk(Color32::from_gray(120)));
    if blue_n > 0 {
        job.append(
            &"\u{2588}".repeat(blue_n),
            0.0,
            mk(Color32::from_rgb(60, 130, 220)),
        );
    }
    if yellow_n > 0 {
        job.append(
            &"\u{2588}".repeat(yellow_n),
            0.0,
            mk(Color32::from_rgb(230, 170, 0)),
        );
    }
    if red_n > 0 {
        job.append(
            &"\u{2588}".repeat(red_n),
            0.0,
            mk(Color32::from_rgb(220, 60, 60)),
        );
    }
    if empty_n > 0 {
        job.append(&"\u{2591}".repeat(empty_n), 0.0, mk(Color32::from_gray(55)));
    }
    job.append("]", 0.0, mk(Color32::from_gray(120)));

    let pct_color = if load >= 70.0 {
        Color32::from_rgb(220, 60, 60)
    } else if load >= 30.0 {
        Color32::from_rgb(230, 170, 0)
    } else {
        Color32::from_rgb(60, 130, 220)
    };
    job.append(&format!(" {load:.1}%"), 0.0, mk(pct_color));

    ui.label(job)
        .on_hover_text("Estimated bus load (fps \u{00d7} 125 bits \u{00f7} baud rate)");
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
            ui.strong(format!("{} RustyCAN", icons::APP));
            ui.separator();
            // Green plug = actively connected
            ui.label(egui::RichText::new(icons::PORT).color(Color32::from_rgb(60, 200, 90)));
            ui.label(&view.form.port);
            ui.label("·");
            ui.label(egui::RichText::new(icons::BAUD));
            ui.label(format!("{} bps", format_bps(&view.form.baud)));
            ui.label("·");
            ui.label(egui::RichText::new(icons::NODES));
            ui.label(format!("{} node(s)", view.form.nodes.len()));
            if view.listen_only {
                ui.separator();
                ui.colored_label(Color32::YELLOW, "LISTEN-ONLY");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(format!("{} Disconnect", icons::DISCONNECT))
                    .clicked()
                {
                    disconnect_clicked = true;
                }
            });
        });
    });

    // ── Bottom status bar ─────────────────────────────────────────────────
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            let log_full = &view.state.log_path;
            let log_display = std::path::Path::new(log_full)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| log_full.clone());

            // FPS
            ui.label(format!(
                "{} {:.1} fps   Total: {}",
                icons::FPS,
                view.state.fps,
                view.state.total_frames,
            ));

            // Bus load — block-character bar
            ui.separator();
            bus_load_bar(ui, view.state.bus_load);

            // Log path
            ui.separator();
            let log_label = ui.label(format!("{} {}", icons::LOG, log_display));
            if !log_full.is_empty() {
                log_label.on_hover_text(log_full);
            }

            if view.disconnected {
                ui.separator();
                ui.colored_label(Color32::RED, "⚠ Adapter disconnected");
            }
        });
    });

    // ── Central panel ─────────────────────────────────────────────────────
    egui::CentralPanel::default().show(ctx, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            nmt_section(
                ui,
                &view.state,
                &view.node_eds_paths,
                &view.cmd_tx,
                view.listen_only,
            );
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

/// NMT action button definitions: (display label, icon, command, fill colour).
fn nmt_action_props() -> [(&'static str, &'static str, NmtCommand, Color32); 5] {
    [
        (
            "Start",
            icons::ACT_START,
            NmtCommand::StartRemoteNode,
            Color32::from_rgb(34, 120, 34),
        ),
        (
            "Stop",
            icons::ACT_STOP,
            NmtCommand::StopRemoteNode,
            Color32::from_rgb(180, 45, 45),
        ),
        (
            "Pre-Op",
            icons::ACT_PREOP,
            NmtCommand::EnterPreOperational,
            Color32::from_rgb(160, 120, 0),
        ),
        (
            "Reset",
            icons::ACT_RESET,
            NmtCommand::ResetNode,
            Color32::from_rgb(170, 80, 0),
        ),
        (
            "Reset Comm",
            icons::ACT_RESET_COMM,
            NmtCommand::ResetCommunication,
            Color32::from_rgb(40, 90, 180),
        ),
    ]
}

fn nmt_section(
    ui: &mut egui::Ui,
    state: &AppState,
    node_eds_paths: &std::collections::HashMap<u8, String>,
    cmd_tx: &mpsc::Sender<CanCommand>,
    listen_only: bool,
) {
    // Header labels for action columns.
    // Indices 3 and 4 form the "Reset" group: row-1 shows "Reset" / "",
    // row-2 shows "Node" / "Comm" below it.
    const HDR1: [&str; 5] = ["Start", "Stop", "Pre-Op", "Reset", ""];
    const HDR2: [&str; 5] = ["", "", "", "Node", "Comm"];

    egui::CollapsingHeader::new(
        egui::RichText::new(format!("{} NMT Status", icons::NMT_HEADER)).strong(),
    )
    .default_open(true)
    .show(ui, |ui| {
        let actions = nmt_action_props();

        egui::Grid::new("nmt_grid")
            .striped(true)
            .min_col_width(0.0)
            .spacing([6.0, 4.0])
            .show(ui, |ui| {
                // ── Header row 1 ─────────────────────────────────────────────
                ui.strong("Node");
                ui.strong("EDS");
                ui.add_sized(
                    vec2(STATE_COL_W, 0.0),
                    egui::Label::new(egui::RichText::new("State").strong()),
                );
                if !listen_only {
                    for label in HDR1 {
                        ui.add_sized(
                            vec2(NMT_COL_W, 0.0),
                            egui::Label::new(egui::RichText::new(label).strong()),
                        );
                    }
                }
                ui.strong("Last seen");
                ui.end_row();

                // ── Header row 2 (Reset sub-labels) ──────────────────────────
                if !listen_only {
                    ui.label("");
                    ui.label("");
                    ui.add_sized(vec2(STATE_COL_W, 0.0), egui::Label::new(""));
                    for label in HDR2 {
                        ui.add_sized(
                            vec2(NMT_COL_W, 0.0),
                            egui::Label::new(
                                egui::RichText::new(label)
                                    .weak()
                                    .size(12.0)
                                    .color(Color32::from_gray(160)),
                            ),
                        );
                    }
                    ui.label("");
                    ui.end_row();
                }

                // ── Broadcast row ─────────────────────────────────────────────
                if !listen_only {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(
                            egui::RichText::new(icons::BROADCAST)
                                .color(Color32::from_rgb(200, 180, 60))
                                .size(14.0),
                        );
                        ui.label(
                            egui::RichText::new("ALL")
                                .strong()
                                .color(Color32::from_rgb(220, 200, 80)),
                        );
                    });
                    ui.label("—");
                    ui.add_sized(vec2(STATE_COL_W, 24.0), egui::Label::new("—"));
                    for (btn_label, icon, cmd, btn_color) in &actions {
                        if ui
                            .add_sized(
                                vec2(NMT_COL_W, 28.0),
                                Button::new(egui::RichText::new(*icon).size(16.0)).fill(*btn_color),
                            )
                            .on_hover_text(format!("Broadcast: {btn_label}"))
                            .clicked()
                        {
                            let _ = cmd_tx.send(CanCommand::SendNmt {
                                command: cmd.clone(),
                                target_node: 0x00,
                            });
                        }
                    }
                    ui.label("—");
                    ui.end_row();
                }

                // ── Node rows ─────────────────────────────────────────────────
                let mut ids: Vec<u8> = state.node_map.keys().copied().collect();
                ids.sort();

                for id in ids {
                    let (eds_name, nmt_state, last_seen) = &state.node_map[&id];
                    let age = last_seen
                        .map(|(t, period)| {
                            let s = t.elapsed().as_secs_f64();
                            let age_str = if s < 60.0 {
                                format!("{s:.1}s ago")
                            } else {
                                format!("{:.0}m ago", s / 60.0)
                            };
                            match period {
                                Some(p) => {
                                    format!("{age_str}  [Δ {:.0}ms]", p.as_secs_f64() * 1000.0)
                                }
                                None => age_str,
                            }
                        })
                        .unwrap_or_else(|| "—".into());

                    let (state_icon, lbl, icon_color, text_color) = nmt_badge(nmt_state);
                    // Node ID: italic, small
                    ui.label(egui::RichText::new(format!("{id}")).italics().size(13.0));
                    // EDS cell: show filename italic+small, hover shows full path
                    {
                        let full_path = node_eds_paths.get(&id).map(|s| s.as_str()).unwrap_or("");
                        let display = if full_path.is_empty() {
                            eds_name.as_str().to_owned()
                        } else {
                            std::path::Path::new(full_path)
                                .file_name()
                                .map(|f| f.to_string_lossy().into_owned())
                                .unwrap_or_else(|| eds_name.as_str().to_owned())
                        };
                        let lbl = ui.label(egui::RichText::new(display).italics().size(13.0));
                        if !full_path.is_empty() {
                            lbl.on_hover_text(full_path);
                        }
                    }
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.add_sized(
                            vec2(18.0, 18.0),
                            egui::Label::new(
                                egui::RichText::new(state_icon).color(icon_color).size(14.0),
                            ),
                        );
                        ui.label(egui::RichText::new(lbl).color(text_color).strong());
                    });
                    if !listen_only {
                        for (btn_label, icon, cmd, btn_color) in &actions {
                            if ui
                                .add_sized(
                                    vec2(NMT_COL_W, 28.0),
                                    Button::new(egui::RichText::new(*icon).size(16.0))
                                        .fill(*btn_color),
                                )
                                .on_hover_text(*btn_label)
                                .clicked()
                            {
                                let _ = cmd_tx.send(CanCommand::SendNmt {
                                    command: cmd.clone(),
                                    target_node: id,
                                });
                            }
                        }
                    }
                    ui.label(age);
                    ui.end_row();
                }

                if state.node_map.is_empty() {
                    ui.label("No nodes detected yet.");
                    ui.end_row();
                }
            });
    });
}

/// Return `(icon, label, icon_color, text_color)` for an NMT state.
/// The icon carries the semantic color; the label uses a softer readable shade.
fn nmt_badge(state: &NmtState) -> (&'static str, &'static str, Color32, Color32) {
    match state {
        NmtState::Operational => (
            icons::STATE_OP,
            "OP",
            Color32::from_rgb(0, 210, 90),    // bright green dot
            Color32::from_rgb(120, 230, 140), // softer green text
        ),
        NmtState::PreOperational => (
            icons::STATE_PREOP,
            "PRE-OP",
            Color32::from_rgb(230, 190, 0),  // amber dot
            Color32::from_rgb(240, 210, 80), // lighter amber text
        ),
        NmtState::Stopped => (
            icons::STATE_STOP,
            "STOP",
            Color32::from_rgb(220, 50, 50),   // red dot
            Color32::from_rgb(240, 120, 120), // lighter red text
        ),
        NmtState::Bootup => (
            icons::STATE_BOOT,
            "BOOT",
            Color32::from_rgb(60, 140, 255),  // blue dot
            Color32::from_rgb(130, 190, 255), // lighter blue text
        ),
        NmtState::Unknown(_) => (
            icons::STATE_UNK,
            "UNKNOWN",
            Color32::GRAY,
            Color32::from_gray(160),
        ),
    }
}

// ─── PDO Live Values section ─────────────────────────────────────────────────

fn pdo_section(ui: &mut egui::Ui, state: &AppState) {
    egui::CollapsingHeader::new(
        egui::RichText::new(format!("{} PDO Live Values", icons::PDO_HEADER)).strong(),
    )
    .default_open(true)
    .show(ui, |ui| {
        if state.pdo_values.is_empty() {
            ui.label("No PDO frames received yet.");
            return;
        }

        let mut keys: Vec<(u8, u16)> = state.pdo_values.keys().copied().collect();
        keys.sort();

        for (node_id, cob_id) in keys {
            if let Some((values, updated, period)) = state.pdo_values.get(&(node_id, cob_id)) {
                let age_secs = updated.elapsed().as_secs_f64();
                let age_str = if age_secs < 60.0 {
                    format!("{age_secs:.2}s ago")
                } else {
                    format!("{:.0}m ago", age_secs / 60.0)
                };
                let age_str = match period {
                    Some(p) => {
                        format!("{age_str}  [Δ {:.0}ms]", p.as_secs_f64() * 1000.0)
                    }
                    None => age_str,
                };

                let header = format!("Node {:3}   COB-ID 0x{:03X}   {}", node_id, cob_id, age_str);
                egui::CollapsingHeader::new(header)
                    .id_salt((node_id, cob_id))
                    .default_open(false)
                    .show(ui, |ui| {
                        egui::Grid::new((node_id, cob_id))
                            .striped(true)
                            .min_col_width(80.0)
                            .show(ui, |ui| {
                                ui.strong("Signal");
                                ui.strong("Value");
                                ui.end_row();
                                for v in values {
                                    ui.label(v.signal_name.as_str());
                                    pdo_value_ui(ui, &v.value);
                                    ui.end_row();
                                }
                            });
                    });
            }
        }
    });
}

// ─── PDO value cell (main value + hex annotation) ────────────────────────────────

fn pdo_value_ui(ui: &mut egui::Ui, val: &PdoRawValue) {
    let hex: Option<String> = match val {
        PdoRawValue::Integer(v) => Some(format!("0x{:X}", *v as u64)),
        PdoRawValue::Unsigned(v) => Some(format!("0x{:X}", v)),
        PdoRawValue::Float(v) => {
            // If the value round-trips through f32, show 4-byte bits (REAL32);
            // otherwise show the full 8-byte f64 bit pattern (REAL64).
            let as_f32 = *v as f32;
            if (as_f32 as f64).to_bits() == v.to_bits() {
                Some(format!("0x{:08X}", as_f32.to_bits()))
            } else {
                Some(format!("0x{:016X}", v.to_bits()))
            }
        }
        PdoRawValue::Text(s) => {
            let hex_str = s
                .bytes()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ");
            Some(hex_str)
        }
        PdoRawValue::Bytes(_) => None, // already displayed as hex by Display
    };

    if let Some(hex_str) = hex {
        ui.horizontal(|ui| {
            ui.label(val.to_string());
            ui.label(
                egui::RichText::new(format!("[{hex_str}]"))
                    .italics()
                    .small()
                    .color(egui::Color32::from_gray(160)),
            );
        });
    } else {
        ui.label(val.to_string());
    }
}

// ─── SDO Log section ──────────────────────────────────────────────────────────

fn sdo_section(ui: &mut egui::Ui, state: &AppState) {
    let title = format!(
        "{} SDO Log  (last {})",
        icons::SDO_HEADER,
        state.sdo_log.len()
    );
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
    let icon = eframe::icon_data::from_png_bytes(include_bytes!(
        "../../assets/RustyCAN.iconset/icon_512x512@2x.png"
    ))
    .expect("bundled icon is valid PNG");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("RustyCAN  {}", env!("RUSTYCAN_VERSION")))
            .with_inner_size([1100.0, 750.0])
            .with_min_inner_size([640.0, 560.0])
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        "RustyCAN",
        options,
        Box::new(|cc| {
            // ── Load MesloLGS NF and set it as the default font ───────────
            let font_data =
                egui::FontData::from_static(include_bytes!("../../assets/MesloLGSNF-Regular.ttf"));
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("meslo_nf".into(), std::sync::Arc::new(font_data));
            // Slot 0 in both families so it is tried first.
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "meslo_nf".into());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, "meslo_nf".into());
            cc.egui_ctx.set_fonts(fonts);

            // ── Bump text sizes ───────────────────────────────────────────
            cc.egui_ctx.style_mut(|s| {
                use egui::{FontId, TextStyle};
                s.text_styles.insert(
                    TextStyle::Body,
                    FontId::new(16.0, egui::FontFamily::Proportional),
                );
                s.text_styles.insert(
                    TextStyle::Heading,
                    FontId::new(20.0, egui::FontFamily::Proportional),
                );
                s.text_styles.insert(
                    TextStyle::Button,
                    FontId::new(15.0, egui::FontFamily::Proportional),
                );
                s.text_styles.insert(
                    TextStyle::Monospace,
                    FontId::new(15.0, egui::FontFamily::Monospace),
                );
                s.text_styles.insert(
                    TextStyle::Small,
                    FontId::new(13.0, egui::FontFamily::Proportional),
                );
            });

            Ok(Box::new(RustyCanApp::new()))
        }),
    )
}
