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
//! - **Automatic adapter fallback** — if the configured adapter is not found,
//!   automatically tries other available adapter types (PEAK ↔ KCAN) and
//!   displays a notice. Manually switching adapters clears the notice.
//! - **Listen-only mode** — checkbox that sets [`SessionConfig::listen_only`];
//!   all [`CanCommand`] variants are silently dropped in the recv thread so
//!   no frames are ever transmitted.
//! - **Node ID from EDS** — when the user browses to an EDS file, the Node ID
//!   box is pre-filled from `[DeviceComissioning] NodeId` if that key exists.
//!   Accepted formats: decimal (`5`), `0x`-prefix hex (`0x05`), `H`/`h`-suffix
//!   hex (`05H`). The box remains freely editable.
//! - **EDS optional** — leaving the EDS path blank is allowed; the node will
//!   appear in the NMT table and any PDO frames will show as raw Byte0…ByteN.
//! - **CANopen Nodes optional** — nodes with empty IDs are filtered out; zero
//!   CANopen nodes is valid when using DBC files or raw frame monitoring only.
//! - **Configuration persistence** — form state (port, baud, nodes, DBC files,
//!   etc.) is saved to `~/Library/Application Support/RustyCAN/config.json` on
//!   successful connect and restored on app launch. Non-existent file paths
//!   are silently filtered out during load.
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
use std::time::{Instant, SystemTime};

use eframe::egui::{self, vec2, Button, Color32};
use rfd::FileDialog;

use crate::adapters::{kcan, AdapterKind};
use crate::app::{apply_event, AppState, CanEvent};
use crate::canopen::nmt::{NmtCommand, NmtState};
use crate::canopen::pdo::PdoRawValue;
use crate::canopen::sdo::{
    encode_value_for_type, parse_hex_bytes, SdoDirection, SdoTransferMode, SdoValue,
};
use crate::eds;
use crate::http_server::SseServer;
use crate::session::{self, CanCommand, SessionConfig};

mod plot_view;

// ─── Icon glyphs (Font Awesome codepoints present in MesloLGS NF) ────────────
mod icons {
    // App / toolbar
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
    pub const SDO_BROWSER: &str = "\u{f002}"; // search
    pub const DBC_HEADER: &str = "\u{f1c9}"; // file-code-o
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
    pub const INFO: &str = "\u{f05a}"; // info-circle
    pub const DASHBOARD: &str = "\u{f0ac}"; // globe
    pub const PLOT: &str = "\u{f201}"; // line-chart
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
    logo: egui::TextureHandle,
    sse_server: SseServer,
    http_port: u16,
}

#[allow(clippy::large_enum_variant)]
enum Screen {
    Connect(ConnectForm),
    Monitor(Box<MonitorView>),
}

impl RustyCanApp {
    fn new(
        cc: &eframe::CreationContext,
        config_path: Option<std::path::PathBuf>,
        http_port: u16,
    ) -> Self {
        let icon_bytes = include_bytes!("../../assets/RustyCAN.iconset/icon_256x256.png");
        let icon_data =
            eframe::icon_data::from_png_bytes(icon_bytes).expect("bundled icon is valid PNG");
        let color_image = egui::ColorImage::from_rgba_unmultiplied(
            [icon_data.width as usize, icon_data.height as usize],
            &icon_data.rgba,
        );
        let logo = cc
            .egui_ctx
            .load_texture("app_logo", color_image, egui::TextureOptions::LINEAR);
        let sse_server = SseServer::start(http_port);

        let screen = if let Some(ref path) = config_path {
            match PersistedConfig::load_from(path) {
                Some(config) => {
                    let mut form = config.into_form();
                    match form.try_connect(sse_server.tx.clone()) {
                        Ok(monitor) => Screen::Monitor(Box::new(monitor)),
                        Err(msg) => {
                            form.error = Some(msg);
                            Screen::Connect(form)
                        }
                    }
                }
                None => {
                    let form = ConnectForm {
                        error: Some(format!("Failed to load config file: {}", path.display())),
                        ..ConnectForm::default()
                    };
                    Screen::Connect(form)
                }
            }
        } else {
            Screen::Connect(ConnectForm::default())
        };

        RustyCanApp {
            screen,
            logo,
            sse_server,
            http_port,
        }
    }
}

impl eframe::App for RustyCanApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint();

        let mut next_screen: Option<Screen> = None;

        match &mut self.screen {
            Screen::Connect(form) => {
                if let Some(s) = render_connect(
                    ctx,
                    form,
                    &self.logo,
                    self.sse_server.tx.clone(),
                    self.http_port,
                ) {
                    next_screen = Some(s);
                }
            }
            Screen::Monitor(view) => {
                if let Some(s) = render_monitor(ctx, view.as_mut(), &self.logo, self.http_port) {
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

#[derive(Clone)]
enum MessageType {
    Error,
    Warning,
    Notice,
}

#[derive(Clone)]
struct MessageEntry {
    text: String,
    msg_type: MessageType,
    timestamp: SystemTime,
    created: Instant,
}

struct ConnectForm {
    port: String,
    baud: String,
    nodes: Vec<NodeEntry>,
    log_path: String,
    /// SDO response timeout in milliseconds.
    sdo_timeout_str: String,
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
    /// If true, also write a plain-text `.log` file alongside the JSONL file.
    text_log: bool,
    /// Which adapter backend to use.
    adapter_kind: AdapterKind,
    /// KCAN devices found during the last USB scan (serial, display name).
    kcan_devices: Vec<(String, String)>,
    /// Selected KCAN device serial (empty = auto-select first).
    kcan_serial: String,
    /// DBC files to load for signal decoding.
    dbc_files: Vec<DbcEntry>,
    /// Index of the DBC file awaiting remove-confirmation, if any.
    confirm_remove_dbc: Option<usize>,
    /// Notice message to display (e.g., adapter auto-switch notification).
    adapter_notice: Option<String>,
    /// Originally configured adapter before any auto-switching.
    original_adapter_kind: Option<AdapterKind>,
    /// Message history (last 5 errors/warnings/notices) for display.
    message_history: Vec<MessageEntry>,
}

impl Clone for ConnectForm {
    fn clone(&self) -> Self {
        ConnectForm {
            port: self.port.clone(),
            baud: self.baud.clone(),
            nodes: self.nodes.clone(),
            log_path: self.log_path.clone(),
            sdo_timeout_str: self.sdo_timeout_str.clone(),
            error: self.error.clone(),
            warnings: self.warnings.clone(),
            confirm_remove: self.confirm_remove,
            dongle_connected: self.dongle_connected,
            // Probe state is not preserved across clones; the new form will
            // start its own probe cycle on the next render frame.
            probe_rx: None,
            last_probe: None,
            listen_only: self.listen_only,
            text_log: self.text_log,
            adapter_kind: self.adapter_kind.clone(),
            kcan_devices: self.kcan_devices.clone(),
            kcan_serial: self.kcan_serial.clone(),
            dbc_files: self.dbc_files.clone(),
            confirm_remove_dbc: self.confirm_remove_dbc,
            adapter_notice: self.adapter_notice.clone(),
            original_adapter_kind: self.original_adapter_kind.clone(),
            message_history: self.message_history.clone(),
        }
    }
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
struct NodeEntry {
    id_str: String,
    eds_path: String,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct DbcEntry {
    path: String,
}

// ── Configuration persistence ────────────────────────────────────────────────

/// Subset of ConnectForm that is persisted to disk between sessions.
/// Excludes runtime state (probe results, errors, confirmation dialogs).
#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedConfig {
    port: String,
    baud: String,
    nodes: Vec<NodeEntry>,
    /// Omitting this field or setting it to null/empty uses the default
    /// `rustycan.jsonl` in the current working directory.
    #[serde(default)]
    log_path: Option<String>,
    sdo_timeout_str: String,
    listen_only: bool,
    text_log: bool,
    adapter_kind: AdapterKind,
    kcan_serial: String,
    dbc_files: Vec<DbcEntry>,
    /// Port for the live HTML dashboard (`http://127.0.0.1:<port>/`).
    /// Omitting this field uses 7878. The `--http-port` CLI flag overrides it.
    #[serde(default)]
    http_port: Option<u16>,
}

impl PersistedConfig {
    /// Get the config file path in the platform-specific app data directory.
    fn config_path() -> std::path::PathBuf {
        let mut path = if cfg!(target_os = "macos") {
            dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from("."))
        } else if cfg!(target_os = "linux") {
            dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from("."))
        } else {
            // Windows
            dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from("."))
        };
        path.push("RustyCAN");
        std::fs::create_dir_all(&path).ok();
        path.push("config.json");
        path
    }

    /// Save configuration to disk as JSON.
    fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {e}"))?;
        std::fs::write(&path, json)
            .map_err(|e| format!("Failed to write config to {}: {e}", path.display()))?;
        Ok(())
    }

    /// Load configuration from disk, filtering out non-existent file paths.
    fn load() -> Option<Self> {
        let path = Self::config_path();
        Self::load_from(&path)
    }

    /// Load configuration from an arbitrary path (e.g. supplied via `--config`).
    ///
    /// Returns `None` if the file cannot be read or parsed. Non-existent EDS
    /// and DBC paths are silently removed so the session can still start.
    fn load_from(path: &std::path::Path) -> Option<Self> {
        let json = std::fs::read_to_string(path).ok()?;
        let mut config: PersistedConfig = serde_json::from_str(&json).ok()?;

        // Filter out nodes with non-existent EDS files
        config.nodes.retain(|node| {
            if node.eds_path.trim().is_empty() {
                return true; // Keep nodes without EDS paths
            }
            std::path::Path::new(&node.eds_path).exists()
        });

        // Filter out non-existent DBC files
        config
            .dbc_files
            .retain(|dbc| std::path::Path::new(&dbc.path).exists());

        Some(config)
    }

    /// Convert to ConnectForm with runtime state initialized to defaults.
    fn into_form(self) -> ConnectForm {
        let log_path = match self.log_path {
            Some(p) if !p.trim().is_empty() => p,
            _ => "rustycan.jsonl".into(),
        };
        ConnectForm {
            port: self.port,
            baud: self.baud,
            nodes: self.nodes,
            log_path,
            sdo_timeout_str: self.sdo_timeout_str,
            error: None,
            warnings: vec![],
            confirm_remove: None,
            dongle_connected: false,
            probe_rx: None,
            last_probe: None,
            listen_only: self.listen_only,
            text_log: self.text_log,
            adapter_kind: self.adapter_kind,
            kcan_devices: vec![],
            kcan_serial: self.kcan_serial,
            dbc_files: self.dbc_files,
            confirm_remove_dbc: None,
            adapter_notice: None,
            original_adapter_kind: None,
            message_history: vec![],
        }
    }
}

impl From<&ConnectForm> for PersistedConfig {
    fn from(form: &ConnectForm) -> Self {
        PersistedConfig {
            port: form.port.clone(),
            baud: form.baud.clone(),
            nodes: form.nodes.clone(),
            log_path: Some(form.log_path.clone()),
            sdo_timeout_str: form.sdo_timeout_str.clone(),
            listen_only: form.listen_only,
            text_log: form.text_log,
            adapter_kind: form.adapter_kind.clone(),
            kcan_serial: form.kcan_serial.clone(),
            dbc_files: form.dbc_files.clone(),
            http_port: None, // not persisted to the app-data config; set via --config file only
        }
    }
}

impl Default for ConnectForm {
    fn default() -> Self {
        // Try to load persisted config; fall back to hardcoded defaults if not found
        if let Some(config) = PersistedConfig::load() {
            config.into_form()
        } else {
            ConnectForm {
                port: "1".into(),
                baud: "250000".into(),
                nodes: vec![],
                log_path: "rustycan.jsonl".into(),
                sdo_timeout_str: "500".into(),
                error: None,
                warnings: vec![],
                confirm_remove: None,
                dongle_connected: false,
                probe_rx: None,
                last_probe: None,
                listen_only: false,
                text_log: false,
                adapter_kind: AdapterKind::Peak,
                kcan_devices: vec![],
                kcan_serial: String::new(),
                dbc_files: vec![],
                confirm_remove_dbc: None,
                adapter_notice: None,
                original_adapter_kind: None,
                message_history: vec![],
            }
        }
    }
}

impl ConnectForm {
    /// Validate the form and start a session.
    fn try_connect(
        &self,
        sse_tx: tokio::sync::broadcast::Sender<String>,
    ) -> Result<MonitorView, String> {
        let baud: u32 = self
            .baud
            .trim()
            .parse()
            .map_err(|_| format!("Invalid baud rate: {:?}", self.baud))?;

        let sdo_timeout_ms: u64 = self
            .sdo_timeout_str
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|&v| v > 0)
            .ok_or_else(|| {
                format!(
                    "SDO timeout must be a positive integer (ms), got {:?}",
                    self.sdo_timeout_str
                )
            })?;

        let nodes: Vec<(u8, Option<PathBuf>)> = self
            .nodes
            .iter()
            .filter(|e| !e.id_str.trim().is_empty())
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
            text_log: self.text_log,
            sdo_timeout_ms,
            block_initiate_timeout_ms: 1000,
            block_subblock_timeout_ms: 500,
            block_end_timeout_ms: 1000,
            block_size: 64,
            adapter_kind: self.adapter_kind.clone(),
            dbc_paths: self
                .dbc_files
                .iter()
                .filter(|e| !e.path.trim().is_empty())
                .map(|e| PathBuf::from(e.path.trim()))
                .collect(),
            sse_tx: Some(sse_tx),
        };

        let (rx, cmd_tx, node_labels, actual_log_path) = session::start(config)?;

        // Save configuration to disk for next session
        let persisted = PersistedConfig::from(self);
        if let Err(e) = persisted.save() {
            eprintln!("Warning: failed to save config: {e}");
        }

        let mut state = AppState::new(actual_log_path, baud);
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

        // Load ODs into a map for the SDO browser (re-parse; errors non-fatal here).
        let node_ods: std::collections::HashMap<u8, crate::eds::types::ObjectDictionary> = self
            .nodes
            .iter()
            .filter_map(|e| {
                let id = eds::parse_node_id_str(e.id_str.trim())?;
                let path = if e.eds_path.trim().is_empty() {
                    return None;
                } else {
                    std::path::PathBuf::from(e.eds_path.trim())
                };
                let od = eds::parse_eds(&path).ok()?;
                Some((id, od))
            })
            .collect();

        let node_labels_clone = node_labels.clone();

        Ok(MonitorView {
            rx,
            cmd_tx,
            state,
            form: saved_form,
            disconnected: false,
            listen_only: self.listen_only,
            node_eds_paths,
            node_labels: node_labels_clone,
            node_ods,
            sdo_browser: SdoBrowserPanel::default(),
            plot_state: plot_view::PlotState::default(),
            plot_open: false,
        })
    }
}

/// Standard CANopen baud rates supported by PEAK PCAN adapters, in bps.
const BAUD_OPTIONS: &[&str] = &[
    "10000", "20000", "50000", "100000", "125000", "250000", "500000", "800000", "1000000",
];
/// How often to re-probe the dongle (seconds).
const PROBE_INTERVAL_SECS: u64 = 2;

/// Return a human-readable display name for an adapter kind.
fn adapter_display_name(kind: &AdapterKind) -> &'static str {
    match kind {
        AdapterKind::Peak => "PEAK PCAN-USB",
        AdapterKind::KCan { .. } => "KCAN Dongle",
    }
}

/// Try to find an available adapter when the configured one is not found.
/// Returns true if a fallback adapter was found and switched to.
fn try_fallback_adapter(form: &mut ConnectForm) -> bool {
    // Save original adapter kind if not already saved
    if form.original_adapter_kind.is_none() {
        form.original_adapter_kind = Some(form.adapter_kind.clone());
    }

    let port = form.port.trim();
    let baud: u32 = form.baud.trim().parse().unwrap_or(250_000);

    // Try other adapter types
    let fallbacks: Vec<AdapterKind> = match &form.adapter_kind {
        AdapterKind::Peak => vec![AdapterKind::KCan { serial: None }],
        AdapterKind::KCan { .. } => vec![AdapterKind::Peak],
    };

    for fallback_kind in fallbacks {
        if session::probe_adapter_with_kind(&fallback_kind, port, baud) {
            let original_name = adapter_display_name(form.original_adapter_kind.as_ref().unwrap());
            let new_name = adapter_display_name(&fallback_kind);

            form.adapter_kind = fallback_kind;
            form.adapter_notice = Some(format!(
                "{} not found, automatically switched to {}",
                original_name, new_name
            ));
            return true;
        }
    }

    false
}

fn render_connect(
    ctx: &egui::Context,
    form: &mut ConnectForm,
    logo: &egui::TextureHandle,
    sse_tx: tokio::sync::broadcast::Sender<String>,
    http_port: u16,
) -> Option<Screen> {
    // ── Dongle probe cycle ────────────────────────────────────────────────────
    // 1. Drain any pending probe result.
    if let Some(rx) = &form.probe_rx {
        if let Ok(result) = rx.try_recv() {
            if result {
                // Configured adapter found successfully
                form.dongle_connected = true;
                form.adapter_notice = None; // Clear any previous notice
            } else {
                // Configured adapter not found, try fallback
                form.dongle_connected = try_fallback_adapter(form);
            }
            form.probe_rx = None;
        }
    }
    // 2. For KCAN, refresh the device list every probe cycle.
    if matches!(form.adapter_kind, AdapterKind::KCan { .. }) {
        form.kcan_devices = kcan::KCanAdapter::list_devices();
    }
    // 3. Launch a new one-shot probe thread if enough time has elapsed.
    let should_probe = form.probe_rx.is_none()
        && form
            .last_probe
            .map(|t| t.elapsed().as_secs() >= PROBE_INTERVAL_SECS)
            .unwrap_or(true); // never probed yet → probe immediately

    if should_probe {
        let port = form.port.trim().to_string();
        let baud: u32 = form.baud.trim().parse().unwrap_or(250_000);
        let kind = form.adapter_kind.clone();
        let (probe_tx, probe_rx) = mpsc::channel::<bool>();
        std::thread::spawn(move || {
            let _ = probe_tx.send(session::probe_adapter_with_kind(&kind, &port, baud));
        });
        form.probe_rx = Some(probe_rx);
        form.last_probe = Some(Instant::now());
    }
    let mut transition = None;

    // Estimated content height so we can split surplus space equally top/bottom.
    const CONNECT_CONTENT_H: f32 = 500.0;

    // ── Top toolbar (consistent with Monitor screen) ──────────────────────
    egui::TopBottomPanel::top("connect_toolbar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            // Logo and title spanning two lines
            ui.add(
                egui::Image::new(logo)
                    .fit_to_exact_size(egui::vec2(48.0, 48.0))
                    .corner_radius(6.0),
            );
            ui.vertical(|ui| {
                ui.strong(egui::RichText::new("RustyCAN").size(24.0));
                ui.label(
                    egui::RichText::new(env!("RUSTYCAN_VERSION"))
                        .size(10.0)
                        .italics()
                        .color(egui::Color32::from_gray(140)),
                );
            });
            ui.separator();

            // Dongle status indicator (always visible)
            if form.dongle_connected {
                ui.label(egui::RichText::new(icons::PLUG_OK).color(Color32::from_rgb(0, 200, 80)));
                ui.label(egui::RichText::new("Detected").color(Color32::from_rgb(0, 200, 80)));
            } else {
                ui.label(
                    egui::RichText::new(icons::PLUG_FAIL).color(Color32::from_rgb(200, 60, 60)),
                );
                ui.label(egui::RichText::new("Not detected").color(Color32::from_rgb(200, 60, 60)));
            }

            // Right side: dashboard link (greyed) + Connect button
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Connect button
                let has_dupes = !form.warnings.is_empty();
                let can_connect = form.dongle_connected && !has_dupes;
                let resp = ui.add_enabled(
                    can_connect,
                    Button::new(
                        egui::RichText::new(format!("{} Connect", icons::PLUG_OK))
                            .color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(34, 160, 54))
                    .min_size(vec2(120.0, 32.0)),
                );

                if !form.dongle_connected {
                    resp.on_disabled_hover_text("Connect a CAN dongle first");
                } else if has_dupes {
                    resp.on_disabled_hover_text("Fix duplicate node IDs before connecting");
                } else if resp.clicked() {
                    match form.try_connect(sse_tx) {
                        Ok(view) => transition = Some(Screen::Monitor(Box::new(view))),
                        Err(e) => form.error = Some(e),
                    }
                }

                // Dashboard link — greyed out; not live until monitoring starts
                ui.separator();
                let url = format!("http://127.0.0.1:{}/", http_port);
                ui.add(
                    egui::Hyperlink::from_label_and_url(
                        egui::RichText::new(icons::DASHBOARD)
                            .size(22.0)
                            .color(Color32::from_gray(90)),
                        &url,
                    )
                    .open_in_new_tab(true),
                )
                .on_hover_text("Live dashboard is available once monitoring starts");
            });
        });
    });

    // ── Bottom status bar (greyed-out, for visual symmetry with Monitor screen) ──
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        ui.vertical(|ui| {
            // Line 1: Greyed-out Bus Load and FPS
            ui.horizontal(|ui| {
                // Greyed-out Bus load bar
                ui.label(
                    egui::RichText::new("Bus [░░░░░░░░░░░░░░░░░░░░] 0.0%")
                        .monospace()
                        .size(13.0)
                        .color(egui::Color32::from_gray(80)),
                );

                // Greyed-out FPS
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("{} 0.0 fps", icons::FPS))
                        .size(13.0)
                        .color(egui::Color32::from_gray(80)),
                );

                // Right-aligned Total count
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("Total:{}", format_count(0)))
                            .size(13.0)
                            .color(egui::Color32::from_gray(80)),
                    );
                });
            });

            // Line 2: Greyed-out Log path
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{} No log file", icons::LOG))
                        .size(13.0)
                        .color(egui::Color32::from_gray(80)),
                );
            });
        });
    });

    egui::CentralPanel::default().show(ctx, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            let top_pad = ((ui.available_height() - CONNECT_CONTENT_H) / 2.0).max(20.0);
            ui.vertical_centered(|ui| {
                ui.add_space(top_pad);
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
                                // ── Adapter type selector ─────────────────────
                                ui.label("Adapter:");
                                ui.horizontal(|ui| {
                                    let is_peak = matches!(form.adapter_kind, AdapterKind::Peak);
                                    if ui.radio(is_peak, "PEAK PCAN-USB").clicked() {
                                        form.adapter_kind = AdapterKind::Peak;
                                        form.last_probe = None; // force re-probe
                                        form.adapter_notice = None; // clear auto-switch notice
                                        form.original_adapter_kind = None; // reset tracking
                                    }
                                    let is_kcan =
                                        matches!(form.adapter_kind, AdapterKind::KCan { .. });
                                    if ui.radio(is_kcan, "KCAN Dongle ★").clicked() {
                                        let serial = if form.kcan_serial.is_empty() {
                                            None
                                        } else {
                                            Some(form.kcan_serial.clone())
                                        };
                                        form.adapter_kind = AdapterKind::KCan { serial };
                                        form.last_probe = None;
                                        form.adapter_notice = None; // clear auto-switch notice
                                        form.original_adapter_kind = None; // reset tracking
                                    }
                                });
                                ui.end_row();

                                // ── KCAN device picker (only when KCAN selected) ──
                                if matches!(form.adapter_kind, AdapterKind::KCan { .. }) {
                                    ui.label("KCAN device:");
                                    ui.horizontal(|ui| {
                                        if form.kcan_devices.is_empty() {
                                            ui.colored_label(
                                                Color32::from_rgb(200, 60, 60),
                                                "No KCAN dongle found",
                                            );
                                        } else {
                                            let selected_label = form
                                                .kcan_devices
                                                .iter()
                                                .find(|(s, _)| s == &form.kcan_serial)
                                                .map(|(_, n)| n.as_str())
                                                .unwrap_or("Auto (first found)");
                                            egui::ComboBox::from_id_salt("kcan_device_combo")
                                                .selected_text(selected_label)
                                                .show_ui(ui, |ui| {
                                                    if ui
                                                        .selectable_value(
                                                            &mut form.kcan_serial,
                                                            String::new(),
                                                            "Auto (first found)",
                                                        )
                                                        .clicked()
                                                    {
                                                        form.adapter_kind =
                                                            AdapterKind::KCan { serial: None };
                                                    }
                                                    for (serial, name) in &form.kcan_devices {
                                                        let label = if serial.is_empty() {
                                                            name.clone()
                                                        } else {
                                                            format!("{name} [{serial}]")
                                                        };
                                                        if ui
                                                            .selectable_value(
                                                                &mut form.kcan_serial,
                                                                serial.clone(),
                                                                label,
                                                            )
                                                            .clicked()
                                                        {
                                                            form.adapter_kind = AdapterKind::KCan {
                                                                serial: Some(serial.clone()),
                                                            };
                                                        }
                                                    }
                                                });
                                        }
                                    });
                                    ui.end_row();
                                }

                                // Port row — only shown for PEAK
                                if matches!(form.adapter_kind, AdapterKind::Peak) {
                                    ui.label("Port:");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut form.port)
                                            .desired_width(80.0)
                                            .hint_text("1"),
                                    );
                                    ui.end_row();
                                }

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

                                ui.label("SDO timeout (ms):");
                                ui.add(
                                    egui::TextEdit::singleline(&mut form.sdo_timeout_str)
                                        .desired_width(80.0)
                                        .hint_text("500"),
                                );
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

                                ui.label("Logging:");
                                ui.checkbox(&mut form.text_log, "Also write plain-text .log file");
                                ui.end_row();
                            });
                    }); // close Connection CollapsingHeader
                }); // close Connection Frame

            ui.add_space(16.0);

            // ── CANopen node configuration ────────────────────────────────
            egui::Frame::group(ui.style())
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    egui::CollapsingHeader::new(
                        egui::RichText::new("CANopen Nodes").size(20.0).strong(),
                    )
                    .default_open(false)
                    .show(ui, |ui| {
                        let mut to_remove: Option<usize> = None;
                        // Snapshot confirm state before iter_mut() to avoid borrow conflict.
                        let confirming = form.confirm_remove;
                        let mut new_confirm: Option<Option<usize>> = None;

                        egui::Grid::new("nodes_grid")
                            .num_columns(2)
                            .spacing([12.0, 10.0])
                            .show(ui, |ui| {
                                if !form.nodes.is_empty() {
                                    ui.label("Node ID");
                                    ui.label("EDS file path");
                                    ui.end_row();
                                }

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
                                            if ui
                                                .add(
                                                    Button::new(
                                                        egui::RichText::new(icons::REMOVE_NODE)
                                                            .size(16.0)
                                                            .color(Color32::from_rgb(220, 60, 60)),
                                                    )
                                                    .min_size(vec2(36.0, 28.0)),
                                                )
                                                .on_hover_text("Remove this node")
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
                                                    if let Some(id) = eds::parse_node_id(&path) {
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

            ui.add_space(16.0);

            // ── DBC nodes (signal decoders) ───────────────────────────────
            egui::Frame::group(ui.style())
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    egui::CollapsingHeader::new(
                        egui::RichText::new("DBC Nodes").size(20.0).strong(),
                    )
                    .default_open(false)
                    .show(ui, |ui| {
                        let mut to_remove_dbc: Option<usize> = None;
                        let _dbc_count = form.dbc_files.len();
                        let confirming_dbc = form.confirm_remove_dbc;
                        let mut new_confirm_dbc: Option<Option<usize>> = None;

                        egui::Grid::new("dbc_grid")
                            .num_columns(1)
                            .spacing([12.0, 10.0])
                            .show(ui, |ui| {
                                ui.label("DBC file path");
                                ui.end_row();

                                for (i, entry) in form.dbc_files.iter_mut().enumerate() {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if confirming_dbc == Some(i) {
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
                                                    to_remove_dbc = Some(i);
                                                    new_confirm_dbc = Some(None);
                                                }
                                                if ui
                                                    .add_sized(
                                                        vec2(60.0, 28.0),
                                                        Button::new("Cancel"),
                                                    )
                                                    .on_hover_text("Keep this DBC file")
                                                    .clicked()
                                                {
                                                    new_confirm_dbc = Some(None);
                                                }
                                                ui.with_layout(
                                                    egui::Layout::left_to_right(
                                                        egui::Align::Center,
                                                    ),
                                                    |ui| {
                                                        ui.label("Remove this DBC file?");
                                                    },
                                                );
                                                return; // skip browse + field while confirming
                                            }

                                            // ── Normal mode ─────────────────────────────────────
                                            if ui
                                                .add(
                                                    Button::new(
                                                        egui::RichText::new(icons::REMOVE_NODE)
                                                            .size(16.0)
                                                            .color(Color32::from_rgb(220, 60, 60)),
                                                    )
                                                    .min_size(vec2(36.0, 28.0)),
                                                )
                                                .on_hover_text("Remove this DBC file")
                                                .clicked()
                                            {
                                                new_confirm_dbc = Some(Some(i));
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
                                                .on_hover_text("Browse for DBC file…")
                                                .clicked()
                                            {
                                                if let Some(path) = FileDialog::new()
                                                    .add_filter("DBC", &["dbc", "DBC"])
                                                    .set_title("Select DBC file")
                                                    .pick_file()
                                                {
                                                    entry.path =
                                                        path.to_string_lossy().into_owned();
                                                }
                                            }
                                            let dbc_id = egui::Id::new(("dbc_path", i));
                                            let dbc_focused =
                                                ui.ctx().memory(|m| m.has_focus(dbc_id));
                                            let mut dbc_display = if dbc_focused {
                                                entry.path.clone()
                                            } else {
                                                std::path::Path::new(&entry.path)
                                                    .file_name()
                                                    .map(|f| f.to_string_lossy().into_owned())
                                                    .unwrap_or_else(|| entry.path.clone())
                                            };
                                            let dbc_resp = ui.add(
                                                egui::TextEdit::singleline(&mut dbc_display)
                                                    .id(dbc_id)
                                                    .desired_width(f32::INFINITY)
                                                    .hint_text("/path/to/signals.dbc"),
                                            );
                                            if dbc_focused && dbc_resp.changed() {
                                                entry.path = dbc_display;
                                            }
                                            if !entry.path.is_empty() {
                                                dbc_resp.on_hover_text(&entry.path);
                                            }
                                        },
                                    );
                                    ui.end_row();
                                }
                            });

                        if let Some(v) = new_confirm_dbc {
                            form.confirm_remove_dbc = v;
                        }
                        if let Some(i) = to_remove_dbc {
                            form.dbc_files.remove(i);
                            if form.confirm_remove_dbc.is_some() {
                                form.confirm_remove_dbc = None;
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
                            .on_hover_text("Add new DBC file")
                            .clicked()
                        {
                            form.dbc_files.push(DbcEntry {
                                path: String::new(),
                            });
                        }
                    });
                });

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

            // ── Error / warning display ───────────────────────────────────
            // Add current error/warning/notice to history
            if let Some(err) = &form.error {
                if form.message_history.is_empty()
                    || form.message_history.last().unwrap().text != *err
                {
                    form.message_history.push(MessageEntry {
                        text: err.clone(),
                        msg_type: MessageType::Error,
                        timestamp: SystemTime::now(),
                        created: Instant::now(),
                    });
                }
            }
            if let Some(notice) = &form.adapter_notice {
                if form.message_history.is_empty()
                    || form.message_history.last().unwrap().text != *notice
                {
                    form.message_history.push(MessageEntry {
                        text: notice.clone(),
                        msg_type: MessageType::Notice,
                        timestamp: SystemTime::now(),
                        created: Instant::now(),
                    });
                }
            }
            for w in &form.warnings {
                if form.message_history.is_empty()
                    || !form
                        .message_history
                        .iter()
                        .any(|m| m.text == *w && matches!(m.msg_type, MessageType::Warning))
                {
                    form.message_history.push(MessageEntry {
                        text: w.clone(),
                        msg_type: MessageType::Warning,
                        timestamp: SystemTime::now(),
                        created: Instant::now(),
                    });
                }
            }

            // Keep only last 5 messages
            if form.message_history.len() > 5 {
                form.message_history
                    .drain(0..form.message_history.len() - 5);
            }

            // Display message history with greying based on age
            ui.vertical_centered(|ui| {
                if !form.message_history.is_empty() {
                    for msg in &form.message_history {
                        let age_secs = msg.created.elapsed().as_secs_f32();
                        // Grey out after 5 seconds, fully grey by 10 seconds
                        let grey_factor = (age_secs - 5.0).max(0.0) / 5.0;
                        let grey_factor = grey_factor.min(1.0);

                        let (icon, base_color) = match msg.msg_type {
                            MessageType::Error => (icons::ERROR, Color32::from_rgb(220, 60, 60)),
                            MessageType::Warning => (icons::WARN, Color32::from_rgb(220, 170, 0)),
                            MessageType::Notice => (icons::INFO, Color32::from_rgb(60, 160, 220)),
                        };

                        // Interpolate towards grey
                        let grey_color = Color32::from_gray(120);
                        let color = Color32::from_rgb(
                            (base_color.r() as f32 * (1.0 - grey_factor)
                                + grey_color.r() as f32 * grey_factor)
                                as u8,
                            (base_color.g() as f32 * (1.0 - grey_factor)
                                + grey_color.g() as f32 * grey_factor)
                                as u8,
                            (base_color.b() as f32 * (1.0 - grey_factor)
                                + grey_color.b() as f32 * grey_factor)
                                as u8,
                        );

                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(icon).size(16.0).color(color));
                            // Format timestamp as HH:MM:SS
                            let time_display =
                                match msg.timestamp.duration_since(std::time::UNIX_EPOCH) {
                                    Ok(duration) => {
                                        let total_secs = duration.as_secs();
                                        let secs = total_secs % 60;
                                        let mins = (total_secs / 60) % 60;
                                        let hours = (total_secs / 3600) % 24;
                                        format!("[{:02}:{:02}:{:02}]", hours, mins, secs)
                                    }
                                    Err(_) => "[--:--:--]".to_string(),
                                };
                            ui.label(
                                egui::RichText::new(time_display)
                                    .size(11.0)
                                    .color(Color32::from_gray(100)),
                            );
                            ui.label(egui::RichText::new(&msg.text).size(11.0).color(color));
                        });
                    }
                } else {
                    ui.add_space(ui.text_style_height(&egui::TextStyle::Body));
                }
            });

            ui.add_space(top_pad.min(40.0));
        });
    });

    transition
}

// ─── Monitor view ─────────────────────────────────────────────────────────────

/// State for the SDO Browser collapsible panel in the Monitor view.
struct SdoBrowserPanel {
    /// Index into the session's sorted node list.
    selected_node_idx: usize,
    /// If true, show the raw hex-entry form; if false, show EDS-driven form.
    raw_mode: bool,

    // EDS pane state
    filter_str: String,
    selected_od_key: Option<(u16, u8)>,
    write_value_str: String,

    // Raw pane state
    raw_index_str: String,
    raw_subindex_str: String,
    raw_data_str: String,

    /// Last encode/validation error from a write attempt.
    last_error: Option<String>,
}

impl Default for SdoBrowserPanel {
    fn default() -> Self {
        SdoBrowserPanel {
            selected_node_idx: 0,
            raw_mode: false,
            filter_str: String::new(),
            selected_od_key: None,
            write_value_str: String::new(),
            raw_index_str: String::new(),
            raw_subindex_str: "0".into(),
            raw_data_str: String::new(),
            last_error: None,
        }
    }
}

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
    /// Ordered node labels for the SDO browser's node selector.
    node_labels: Vec<(u8, String)>,
    /// Loaded object dictionaries per node, for the SDO browser.
    node_ods: std::collections::HashMap<u8, crate::eds::types::ObjectDictionary>,
    /// State for the SDO Browser panel.
    sdo_browser: SdoBrowserPanel,
    /// All plot-related state (ring buffers, chart configs).
    plot_state: plot_view::PlotState,
    /// Whether the plot window is currently open.
    plot_open: bool,
}

/// Smart truncation of file paths for display.
/// Keeps the filename always visible and uses ellipses in the middle if the path is too long.
fn truncate_path_smart(full_path: &str, ui: &egui::Ui, available_width: f32) -> String {
    if full_path.is_empty() {
        return full_path.to_string();
    }

    // Measure full path width
    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let full_width = ui.fonts(|f| {
        f.layout_no_wrap(full_path.to_string(), font_id.clone(), egui::Color32::WHITE)
            .rect
            .width()
    });

    // If it fits, return as-is
    if full_width <= available_width {
        return full_path.to_string();
    }

    // Extract filename (everything after the last '/')
    let path = std::path::Path::new(full_path);
    let filename = path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(full_path);

    // Reserve space for ellipsis and filename
    let ellipsis = "...";
    let suffix = format!("{}{}", ellipsis, filename);
    let suffix_width = ui.fonts(|f| {
        f.layout_no_wrap(suffix.clone(), font_id.clone(), egui::Color32::WHITE)
            .rect
            .width()
    });

    // If even the filename doesn't fit, just show it
    if suffix_width >= available_width {
        return filename.to_string();
    }

    // Find the parent directory path
    let parent = path.parent().and_then(|p| p.to_str()).unwrap_or("");

    if parent.is_empty() {
        return filename.to_string();
    }

    // Binary search for the right prefix length
    let mut left = 0;
    let mut right = parent.len();
    let mut best_len = 0;

    while left <= right {
        let mid = (left + right) / 2;
        let prefix = &parent[..mid];
        let candidate = format!("{}{}{}", prefix, ellipsis, filename);
        let candidate_width = ui.fonts(|f| {
            f.layout_no_wrap(candidate.clone(), font_id.clone(), egui::Color32::WHITE)
                .rect
                .width()
        });

        if candidate_width <= available_width {
            best_len = mid;
            left = mid + 1;
        } else {
            right = mid.saturating_sub(1);
        }

        if left > right {
            break;
        }
    }

    if best_len == 0 {
        return suffix;
    }

    format!("{}{}{}", &parent[..best_len], ellipsis, filename)
}

/// Format a number with comma separators and leading spaces (fixed width for alignment).
/// Supports up to billions (999,999,999,999).
fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();

    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }

    let formatted: String = result.chars().rev().collect();

    // Pad to 15 characters (for up to 999,999,999,999)
    format!("{:>15}", formatted)
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

fn render_monitor(
    ctx: &egui::Context,
    view: &mut MonitorView,
    logo: &egui::TextureHandle,
    http_port: u16,
) -> Option<Screen> {
    // Drain all pending CAN events; intercept AdapterError before rendering.
    let mut adapter_error: Option<String> = None;
    loop {
        match view.rx.try_recv() {
            Ok(CanEvent::AdapterError(e)) => {
                adapter_error = Some(e);
                break;
            }
            Ok(ev) => {
                let t_secs = SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs_f64();
                view.plot_state.feed_event(&ev, t_secs);
                apply_event(&mut view.state, ev);
            }
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
            // Logo and title spanning two lines
            ui.add(
                egui::Image::new(logo)
                    .fit_to_exact_size(egui::vec2(48.0, 48.0))
                    .corner_radius(6.0),
            );
            ui.vertical(|ui| {
                ui.strong(egui::RichText::new("RustyCAN").size(28.0));
                ui.label(
                    egui::RichText::new(env!("RUSTYCAN_VERSION"))
                        .size(10.0)
                        .italics()
                        .color(egui::Color32::from_gray(140)),
                );
            });
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
                    .add_sized(
                        vec2(120.0, 32.0),
                        Button::new(
                            egui::RichText::new(format!("{} Disconnect", icons::DISCONNECT))
                                .color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(220, 60, 60)),
                    )
                    .on_hover_text("Return to configuration screen")
                    .clicked()
                {
                    disconnect_clicked = true;
                }

                // Dashboard link — active (full colour) while monitoring
                ui.separator();
                let url = format!("http://127.0.0.1:{}/", http_port);
                ui.add(
                    egui::Hyperlink::from_label_and_url(
                        egui::RichText::new(icons::DASHBOARD)
                            .size(22.0)
                            .color(Color32::from_rgb(60, 160, 220)),
                        &url,
                    )
                    .open_in_new_tab(true),
                )
                .on_hover_text(format!("Open live dashboard at {}", url));

                // Plot window toggle button
                ui.separator();
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new(icons::PLOT).size(22.0).color(
                            if view.plot_open {
                                Color32::from_rgb(60, 160, 220)
                            } else {
                                Color32::from_gray(140)
                            },
                        ))
                        .frame(false),
                    )
                    .on_hover_text(if view.plot_open {
                        "Close plot window"
                    } else {
                        "Open plot window"
                    })
                    .clicked()
                {
                    view.plot_open = !view.plot_open;
                }
            });
        });
    });

    // ── Bottom status bar ─────────────────────────────────────────────────
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        ui.vertical(|ui| {
            // Line 1: Bus Load and FPS
            ui.horizontal(|ui| {
                // Bus load — block-character bar
                bus_load_bar(ui, view.state.bus_load);

                // FPS
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("{} {:.1} fps", icons::FPS, view.state.fps,))
                        .size(13.0),
                );

                // Right-aligned Total count
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "Total:{}",
                            format_count(view.state.total_frames)
                        ))
                        .size(13.0),
                    );
                });
            });

            // Line 2: Log path and status
            ui.horizontal(|ui| {
                let log_full = &view.state.log_path;

                // Use smart truncation for the log path (keeps filename visible with ellipses in middle)
                let available_width = ui.available_width() - 100.0; // Reserve space for disconnect warning
                let log_display = truncate_path_smart(log_full, ui, available_width);

                // Log path
                let log_label = ui.label(
                    egui::RichText::new(format!("{} {}", icons::LOG, log_display)).size(13.0),
                );
                if !log_full.is_empty() && log_display != *log_full {
                    log_label.on_hover_text(log_full); // Hover to see full path if truncated
                }

                if view.disconnected {
                    ui.separator();
                    ui.colored_label(
                        Color32::RED,
                        egui::RichText::new("⚠ Adapter disconnected").size(13.0),
                    );
                }
            });
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
            dbc_section(ui, &view.state);
            ui.add_space(4.0);
            sdo_browser_section(
                ui,
                &view.state,
                &view.node_labels,
                &view.node_ods,
                &view.cmd_tx,
                view.listen_only,
                &mut view.sdo_browser,
            );
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

    // ── Plot window (second OS window, immediate viewport) ────────────────
    if view.plot_open {
        let vp_id = egui::ViewportId(egui::Id::new("rustycan_plots"));
        let builder = egui::ViewportBuilder::default()
            .with_title("RustyCAN – Plots")
            .with_inner_size(egui::vec2(1020.0, 660.0));
        let plot_state = &mut view.plot_state;
        let plot_open = &mut view.plot_open;
        ctx.show_viewport_immediate(vp_id, builder, |ctx, class| {
            if ctx.input(|i| i.viewport().close_requested()) {
                *plot_open = false;
            }
            plot_view::render(ctx, class, plot_state);
        });
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

// ─── DBC Signals section ─────────────────────────────────────────────────────

fn dbc_section(ui: &mut egui::Ui, state: &AppState) {
    egui::CollapsingHeader::new(
        egui::RichText::new(format!("{} DBC Signals", icons::DBC_HEADER)).strong(),
    )
    .default_open(true)
    .show(ui, |ui| {
        match &state.dbc_loaded {
            None => {
                ui.label(
                    egui::RichText::new(
                        "(no DBC loaded — browse for a .dbc file on the Connect screen)",
                    )
                    .italics()
                    .color(Color32::from_gray(120)),
                );
            }
            Some(filename) => {
                if state.dbc_signals.is_empty() {
                    ui.label(
                        egui::RichText::new(format!(
                            "DBC: {} — waiting for matching frames…",
                            filename
                        ))
                        .color(Color32::from_gray(160)),
                    );
                    return;
                }

                // Collect and sort signals by (can_id, signal_name)
                let mut entries: Vec<&crate::app::DbcSignalEntry> =
                    state.dbc_signals.values().collect();
                entries.sort_by(|a, b| {
                    a.can_id
                        .cmp(&b.can_id)
                        .then_with(|| a.signal_name.cmp(&b.signal_name))
                });

                egui::Grid::new("dbc_signals_grid")
                    .striped(true)
                    .min_col_width(60.0)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        ui.strong("Message");
                        ui.strong("Signal");
                        ui.strong("Value");
                        ui.strong("Unit");
                        ui.strong("Age");
                        ui.strong("Count");
                        ui.end_row();

                        for entry in entries {
                            let age_secs = entry.last_seen.elapsed().as_secs_f64();
                            let age_str = if age_secs < 60.0 {
                                format!("{age_secs:.2}s ago")
                            } else {
                                format!("{:.0}m ago", age_secs / 60.0)
                            };

                            let value_str =
                                if entry.physical.fract() == 0.0 && entry.physical.abs() < 1e12 {
                                    format!("{}", entry.physical as i64)
                                } else {
                                    format!("{:.4}", entry.physical)
                                };

                            ui.label(format!("0x{:03X} {}", entry.can_id, &entry.message_name))
                                .on_hover_text(format!("CAN ID 0x{:03X}", entry.can_id));
                            ui.label(&entry.signal_name);
                            // If there's a VAL_ description, show it with the raw integer too.
                            let val_lbl = match &entry.description {
                                Some(desc) => {
                                    let lbl = ui
                                        .label(
                                            egui::RichText::new(format!("{value_str} ({desc})"))
                                                .color(Color32::from_rgb(130, 200, 255)),
                                        )
                                        .on_hover_text(format!("raw = {}", entry.raw_int));
                                    lbl
                                }
                                None => ui.label(&value_str),
                            };
                            drop(val_lbl);
                            ui.label(&entry.unit);
                            ui.label(age_str);
                            ui.label(entry.count.to_string());
                            ui.end_row();
                        }
                    });
            }
        }
    });
}

// ─── SDO Browser section ─────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn sdo_browser_section(
    ui: &mut egui::Ui,
    state: &AppState,
    node_labels: &[(u8, String)],
    node_ods: &std::collections::HashMap<u8, crate::eds::types::ObjectDictionary>,
    cmd_tx: &mpsc::Sender<CanCommand>,
    listen_only: bool,
    panel: &mut SdoBrowserPanel,
) {
    use crate::eds::types::AccessType;

    egui::CollapsingHeader::new(
        egui::RichText::new(format!("{} SDO Browser", icons::SDO_BROWSER)).strong(),
    )
    .default_open(false)
    .show(ui, |ui| {
        if listen_only {
            ui.colored_label(
                Color32::YELLOW,
                "SDO Browser is disabled in listen-only mode.",
            );
            return;
        }
        if node_labels.is_empty() {
            ui.label("No nodes configured.");
            return;
        }

        // ── Node selector ─────────────────────────────────────────────────
        panel.selected_node_idx = panel.selected_node_idx.min(node_labels.len() - 1);
        let (current_id, current_label) = &node_labels[panel.selected_node_idx];
        let current_id = *current_id;

        ui.horizontal(|ui| {
            ui.label("Node:");
            egui::ComboBox::from_id_salt("sdo_browser_node")
                .selected_text(format!("{} ({})", current_id, current_label))
                .show_ui(ui, |ui| {
                    for (i, (id, label)) in node_labels.iter().enumerate() {
                        let label_str = format!("{} ({})", id, label);
                        if ui
                            .selectable_value(&mut panel.selected_node_idx, i, label_str)
                            .changed()
                        {
                            panel.selected_od_key = None;
                            panel.write_value_str.clear();
                            panel.last_error = None;
                        }
                    }
                });

            let has_od = node_ods.contains_key(&current_id);

            // EDS / Raw tab buttons
            ui.separator();
            let eds_btn = ui.add_enabled(
                has_od,
                Button::new("EDS").fill(if !panel.raw_mode && has_od {
                    Color32::from_rgb(40, 90, 180)
                } else {
                    Color32::from_gray(55)
                }),
            );
            if has_od && eds_btn.clicked() {
                panel.raw_mode = false;
                panel.last_error = None;
            }
            let raw_btn = ui.add(Button::new("Raw").fill(if panel.raw_mode {
                Color32::from_rgb(40, 90, 180)
            } else {
                Color32::from_gray(55)
            }));
            if raw_btn.clicked() {
                panel.raw_mode = true;
                panel.last_error = None;
            }
            if !has_od && !panel.raw_mode {
                panel.raw_mode = true;
            }

            // Pending indicator
            if let Some((idx, sub, dir)) = state.pending_sdos.get(&current_id) {
                let dir_str = match dir {
                    SdoDirection::Read => "READ",
                    SdoDirection::Write => "WRITE",
                };
                ui.separator();
                ui.colored_label(
                    Color32::from_rgb(230, 190, 0),
                    format!("\u{23f3} Waiting for {dir_str} {:04X}h/{:02X}…", idx, sub),
                );
            }
        });

        let is_pending = state.pending_sdos.contains_key(&current_id);

        // ── Error display ─────────────────────────────────────────────────
        if let Some(err) = &panel.last_error {
            ui.colored_label(Color32::from_rgb(220, 60, 60), format!("\u{26a0} {err}"));
        }

        ui.add_space(4.0);

        if panel.raw_mode {
            // ── Raw pane ──────────────────────────────────────────────────
            egui::Grid::new("sdo_raw_grid")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Index (hex):");
                    ui.add(
                        egui::TextEdit::singleline(&mut panel.raw_index_str)
                            .desired_width(70.0)
                            .hint_text("1001"),
                    );
                    ui.end_row();

                    ui.label("Subindex:");
                    ui.add(
                        egui::TextEdit::singleline(&mut panel.raw_subindex_str)
                            .desired_width(50.0)
                            .hint_text("0"),
                    );
                    ui.end_row();
                });

            ui.horizontal(|ui| {
                let read_ok = !is_pending && !panel.raw_index_str.trim().is_empty();
                if ui
                    .add_enabled(read_ok, Button::new("Read"))
                    .on_disabled_hover_text(if is_pending {
                        "Waiting for previous response"
                    } else {
                        "Enter an index first"
                    })
                    .clicked()
                {
                    match parse_raw_index_sub(&panel.raw_index_str, &panel.raw_subindex_str) {
                        Ok((index, subindex)) => {
                            panel.last_error = None;
                            let _ = cmd_tx.send(CanCommand::SdoRead {
                                node_id: current_id,
                                index,
                                subindex,
                                mode: SdoTransferMode::Auto,
                            });
                        }
                        Err(e) => panel.last_error = Some(e),
                    }
                }
            });

            ui.add_space(4.0);
            ui.label("Write data (hex bytes):");
            ui.add(
                egui::TextEdit::singleline(&mut panel.raw_data_str)
                    .desired_width(f32::INFINITY)
                    .hint_text("01 02 03 04"),
            );
            ui.horizontal(|ui| {
                let write_ok = !is_pending && !panel.raw_index_str.trim().is_empty();
                if ui
                    .add_enabled(write_ok, Button::new("Write"))
                    .on_disabled_hover_text(if is_pending {
                        "Waiting for previous response"
                    } else {
                        "Enter an index first"
                    })
                    .clicked()
                {
                    match parse_raw_index_sub(&panel.raw_index_str, &panel.raw_subindex_str) {
                        Ok((index, subindex)) => match parse_hex_bytes(&panel.raw_data_str) {
                            Ok(data) => {
                                panel.last_error = None;
                                let _ = cmd_tx.send(CanCommand::SdoWrite {
                                    node_id: current_id,
                                    index,
                                    subindex,
                                    data,
                                    mode: SdoTransferMode::Auto,
                                });
                            }
                            Err(e) => panel.last_error = Some(e),
                        },
                        Err(e) => panel.last_error = Some(e),
                    }
                }
            });
        } else {
            // ── EDS pane ──────────────────────────────────────────────────
            if let Some(od) = node_ods.get(&current_id) {
                // Filter text box
                ui.horizontal(|ui| {
                    ui.label("Filter:");
                    ui.add(
                        egui::TextEdit::singleline(&mut panel.filter_str)
                            .desired_width(200.0)
                            .hint_text("name or index…"),
                    );
                    if ui.small_button("✕").clicked() {
                        panel.filter_str.clear();
                    }
                });

                // Collect and filter entries, then group by index.
                let filter = panel.filter_str.to_lowercase();
                let mut entries: Vec<((u16, u8), &crate::eds::types::OdEntry)> = od
                    .iter()
                    .filter(|((idx, _sub), entry)| {
                        if filter.is_empty() {
                            return true;
                        }
                        entry.name.to_lowercase().contains(&filter)
                            || format!("{:04x}", idx).contains(&filter)
                            || format!("{:04X}", idx).contains(&filter)
                    })
                    .map(|(k, v)| (*k, v))
                    .collect();
                entries.sort_by_key(|(k, _)| *k);

                // Build ordered list of unique indexes.
                let mut indexes: Vec<u16> = entries.iter().map(|((idx, _), _)| *idx).collect();
                indexes.dedup();

                // Scrollable tree of index groups → subindex rows.
                egui::ScrollArea::vertical()
                    .id_salt("sdo_browser_od")
                    .max_height(280.0)
                    .show(ui, |ui| {
                        for idx in &indexes {
                            let subs: Vec<(u8, &crate::eds::types::OdEntry)> = entries
                                .iter()
                                .filter(|((i, _), _)| i == idx)
                                .map(|((_, s), e)| (*s, *e))
                                .collect();

                            // Pick a representative name for the group header:
                            // sub 0 name if it exists, otherwise first sub's name.
                            let group_name = subs
                                .iter()
                                .find(|(s, _)| *s == 0)
                                .or_else(|| subs.first())
                                .map(|(_, e)| e.name.as_str())
                                .unwrap_or("");

                            // Highlight the header if any sub in this group is selected.
                            let group_selected = panel
                                .selected_od_key
                                .map(|(si, _)| si == *idx)
                                .unwrap_or(false);

                            // Auto-open when the filter is active or when an entry in
                            // this group is selected.
                            let force_open = !filter.is_empty() || group_selected;

                            let header_text =
                                egui::RichText::new(format!("{:04X}h  {}", idx, group_name)).color(
                                    if group_selected {
                                        Color32::from_rgb(130, 190, 255)
                                    } else {
                                        Color32::GRAY
                                    },
                                );

                            let mut header =
                                egui::CollapsingHeader::new(header_text).id_salt(("sdo_idx", idx));
                            if force_open {
                                header = header.open(Some(true));
                            }
                            header.show(ui, |ui| {
                                // Calculate column widths to fill available space
                                let available_width = ui.available_width();
                                let spacing = 8.0;
                                let sub_width = 40.0;
                                let type_width = 100.0;
                                let access_width = 50.0;
                                let name_width = (available_width
                                    - sub_width
                                    - type_width
                                    - access_width
                                    - spacing * 3.0)
                                    .max(100.0);

                                egui::Grid::new(("sdo_sub_grid", idx))
                                    .striped(true)
                                    .spacing([spacing, 3.0])
                                    .show(ui, |ui| {
                                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);

                                        ui.strong("Sub");
                                        ui.strong("Name");
                                        ui.strong("Type");
                                        ui.strong("Access");
                                        ui.end_row();

                                        for (sub, entry) in &subs {
                                            let key = (*idx, *sub);
                                            let is_selected = panel.selected_od_key == Some(key);

                                            // Apply background highlighting for selected row
                                            if is_selected {
                                                let row_rect = ui.available_rect_before_wrap();
                                                ui.painter().rect_filled(
                                                    row_rect,
                                                    0.0,
                                                    Color32::from_rgba_unmultiplied(
                                                        130, 190, 255, 30,
                                                    ),
                                                );
                                            }

                                            let row_text = |s: &str| {
                                                let t = egui::RichText::new(s);
                                                if is_selected {
                                                    t.strong()
                                                        .color(Color32::from_rgb(130, 190, 255))
                                                } else {
                                                    t
                                                }
                                            };

                                            // Make all cells clickable by wrapping each in selectable_label
                                            let sub_response = ui
                                                .add_sized(
                                                    [sub_width, 0.0],
                                                    egui::SelectableLabel::new(
                                                        is_selected,
                                                        row_text(&format!("{}", sub)),
                                                    ),
                                                )
                                                .on_hover_text(format!(
                                                    "{:04X}h / sub {:02}  —  {}",
                                                    idx, sub, entry.name
                                                ));

                                            let name_response = ui.add_sized(
                                                [name_width, 0.0],
                                                egui::SelectableLabel::new(
                                                    is_selected,
                                                    row_text(entry.name.as_str()),
                                                ),
                                            );

                                            let type_response = ui.add_sized(
                                                [type_width, 0.0],
                                                egui::SelectableLabel::new(
                                                    is_selected,
                                                    row_text(
                                                        &format!("{:?}", entry.data_type)
                                                            .replace("DataType::", ""),
                                                    ),
                                                ),
                                            );

                                            let (acc_str, acc_color) = match &entry.access {
                                                AccessType::ReadOnly | AccessType::Const => {
                                                    ("RO", Color32::from_rgb(80, 200, 255))
                                                }
                                                AccessType::WriteOnly => {
                                                    ("WO", Color32::from_rgb(220, 100, 220))
                                                }
                                                AccessType::ReadWrite => {
                                                    ("R/W", Color32::from_rgb(100, 220, 100))
                                                }
                                                AccessType::Unknown => ("?", Color32::GRAY),
                                            };
                                            let access_response = ui.add_sized(
                                                [access_width, 0.0],
                                                egui::SelectableLabel::new(
                                                    is_selected,
                                                    egui::RichText::new(acc_str).color(acc_color),
                                                ),
                                            );

                                            // Check if any cell was clicked
                                            if sub_response.clicked()
                                                || name_response.clicked()
                                                || type_response.clicked()
                                                || access_response.clicked()
                                            {
                                                if panel.selected_od_key == Some(key) {
                                                    panel.selected_od_key = None;
                                                } else {
                                                    panel.selected_od_key = Some(key);
                                                    panel.write_value_str.clear();
                                                    panel.last_error = None;
                                                }
                                            }

                                            ui.end_row();
                                        }
                                    });
                            });
                        }
                    });

                // Detail + action area for selected entry
                if let Some(key) = panel.selected_od_key {
                    if let Some(entry) = od.get(&key) {
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.strong(&entry.name);
                            ui.label(format!(
                                "— {:04X}h/{:02X}  {:?}",
                                key.0, key.1, entry.data_type
                            ));
                        });

                        let can_read = !matches!(entry.access, AccessType::WriteOnly);
                        let can_write =
                            !matches!(entry.access, AccessType::ReadOnly | AccessType::Const);

                        ui.horizontal(|ui| {
                            // Read button
                            if ui
                                .add_enabled(
                                    can_read && !is_pending,
                                    Button::new("Read").fill(Color32::from_rgb(40, 90, 180)),
                                )
                                .on_disabled_hover_text(if is_pending {
                                    "Waiting for previous response"
                                } else {
                                    "Object is write-only"
                                })
                                .clicked()
                            {
                                panel.last_error = None;
                                let _ = cmd_tx.send(CanCommand::SdoRead {
                                    node_id: current_id,
                                    index: key.0,
                                    subindex: key.1,
                                    mode: SdoTransferMode::Auto,
                                });
                            }

                            // Write area
                            if can_write {
                                let hint = value_hint_for_type(&entry.data_type);
                                ui.add(
                                    egui::TextEdit::singleline(&mut panel.write_value_str)
                                        .desired_width(160.0)
                                        .hint_text(hint),
                                );
                                if ui
                                    .add_enabled(
                                        !is_pending,
                                        Button::new("Write").fill(Color32::from_rgb(120, 45, 130)),
                                    )
                                    .on_disabled_hover_text("Waiting for previous response")
                                    .clicked()
                                {
                                    match encode_value_for_type(
                                        &panel.write_value_str,
                                        &entry.data_type,
                                    ) {
                                        Ok(data) => {
                                            panel.last_error = None;
                                            let _ = cmd_tx.send(CanCommand::SdoWrite {
                                                node_id: current_id,
                                                index: key.0,
                                                subindex: key.1,
                                                data,
                                                mode: SdoTransferMode::Auto,
                                            });
                                        }
                                        Err(e) => panel.last_error = Some(e),
                                    }
                                }
                            }
                        });
                    }
                }
            }
        }
    });
}

/// Return a short placeholder string describing the expected input for a given `DataType`.
fn value_hint_for_type(dtype: &crate::eds::types::DataType) -> &'static str {
    use crate::eds::types::DataType;
    match dtype {
        DataType::Boolean => "true / false",
        DataType::Integer8 => "-128 … 127",
        DataType::Integer16 => "-32768 … 32767",
        DataType::Integer32 => "-2147483648 … 2147483647",
        DataType::Integer64 => "signed 64-bit",
        DataType::Unsigned8 => "0 … 255",
        DataType::Unsigned16 => "0 … 65535  or  0x…",
        DataType::Unsigned32 => "0 … 4294967295  or  0x…",
        DataType::Unsigned64 => "unsigned 64-bit  or  0x…",
        DataType::Real32 => "e.g. 3.14",
        DataType::Real64 => "e.g. 3.141592",
        DataType::VisibleString => "text string",
        DataType::OctetString | DataType::Unknown(_) => "hex bytes: 01 02 …",
    }
}

/// Parse the raw index (hex) and subindex (decimal) fields from the browser.
fn parse_raw_index_sub(index_str: &str, sub_str: &str) -> Result<(u16, u8), String> {
    let s = index_str.trim();
    let index = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16).map_err(|_| format!("Invalid hex index {:?}", index_str))?
    } else {
        // Try hex without prefix first (common for OD indices)
        u16::from_str_radix(s, 16)
            .or_else(|_| s.parse::<u16>())
            .map_err(|_| {
                format!(
                    "Invalid index {:?} (use hex like 1001 or 0x1001)",
                    index_str
                )
            })?
    };
    let subindex = sub_str
        .trim()
        .parse::<u8>()
        .map_err(|_| format!("Invalid subindex {:?} (decimal 0–255)", sub_str))?;
    Ok((index, subindex))
}

// ─── SDO Log section ──────────────────────────────────────────────────────────

/// Render an SDO value with both main format and hex annotation (like PDO display).
fn sdo_value_ui(ui: &mut egui::Ui, val: &SdoValue) {
    let hex: Option<String> = match val {
        SdoValue::I8(v) => Some(format!("0x{:02X}", *v as u8)),
        SdoValue::I16(v) => Some(format!("0x{:04X}", *v as u16)),
        SdoValue::I32(v) => Some(format!("0x{:08X}", *v as u32)),
        SdoValue::I64(v) => Some(format!("0x{:016X}", *v as u64)),
        SdoValue::U8(v) => Some(format!("0x{:02X}", v)),
        SdoValue::U16(v) => Some(format!("0x{:04X}", v)),
        SdoValue::U32(v) => Some(format!("0x{:08X}", v)),
        SdoValue::U64(v) => Some(format!("0x{:016X}", v)),
        SdoValue::F32(v) => Some(format!("0x{:08X}", v.to_bits())),
        SdoValue::F64(v) => Some(format!("0x{:016X}", v.to_bits())),
        SdoValue::Bytes(b) => {
            // For byte arrays, try to show as ASCII if printable (excluding null terminators)
            let as_str: Option<String> = if b.iter().all(|&byte| {
                byte == 0
                    || byte == 0x09
                    || byte == 0x0A
                    || byte == 0x0D
                    || (0x20..0x7F).contains(&byte)
            }) {
                // Strip trailing null bytes and convert to string
                let trimmed = b
                    .iter()
                    .take_while(|&&byte| byte != 0)
                    .copied()
                    .collect::<Vec<u8>>();
                String::from_utf8(trimmed).ok()
            } else {
                None
            };

            // Always generate hex string for display
            let hex_str = b
                .iter()
                .map(|byte| format!("{:02X}", byte))
                .collect::<Vec<_>>()
                .join(" ");

            if let Some(text) = as_str {
                // For visible strings: show string first, then hex bytes in italics
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("\"{}\"", text));
                    ui.label(
                        egui::RichText::new(hex_str)
                            .italics()
                            .small()
                            .color(egui::Color32::from_gray(160)),
                    );
                });
                return;
            } else {
                // For non-printable bytes: show hex format in brackets
                ui.label(format!("[{}]", hex_str));
                return;
            }
        }
        SdoValue::Bool(_) => None, // "true"/"false" doesn't need hex
    };

    if let Some(hex_str) = hex {
        ui.horizontal_wrapped(|ui| {
            // Show decimal/main value
            let main_value = match val {
                SdoValue::I8(v) => format!("{}", v),
                SdoValue::I16(v) => format!("{}", v),
                SdoValue::I32(v) => format!("{}", v),
                SdoValue::I64(v) => format!("{}", v),
                SdoValue::U8(v) => format!("{}", v),
                SdoValue::U16(v) => format!("{}", v),
                SdoValue::U32(v) => format!("{}", v),
                SdoValue::U64(v) => format!("{}", v),
                SdoValue::F32(v) => format!("{:.4}", v),
                SdoValue::F64(v) => format!("{:.6}", v),
                _ => val.to_string(),
            };
            ui.label(main_value);
            ui.label(
                egui::RichText::new(format!("[{}]", hex_str))
                    .italics()
                    .small()
                    .color(egui::Color32::from_gray(160)),
            );
        });
    } else {
        ui.label(val.to_string());
    }
}

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
                                ui.label("=");
                                sdo_value_ui(ui, v);
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
pub fn run(config_path: Option<std::path::PathBuf>, http_port: u16) -> Result<(), eframe::Error> {
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

            Ok(Box::new(RustyCanApp::new(cc, config_path, http_port)))
        }),
    )
}

// ─── Public helper for non-GUI entry points ───────────────────────────────────

/// Load a [`crate::session::SessionConfig`] from a JSON config file (the same
/// format written by the GUI and documented in `config.example.json`).
///
/// This function is used by the `--tui` and `--log-to-stdout` CLI modes so
/// they can share the exact same config schema without duplicating any parsing
/// logic.  Pass `Some(sender)` when an SSE broadcast server is already running;
/// pass `None` to disable live-dashboard mirroring.
///
/// # Errors
/// Returns a human-readable error string if the file cannot be read, parsed,
/// or if any required field (e.g. baud rate) is invalid.
pub fn load_session_config(
    path: &std::path::Path,
    sse_tx: Option<tokio::sync::broadcast::Sender<String>>,
) -> Result<crate::session::SessionConfig, String> {
    let config = PersistedConfig::load_from(path)
        .ok_or_else(|| format!("Cannot read or parse config file: {}", path.display()))?;

    let baud: u32 = config
        .baud
        .trim()
        .parse()
        .map_err(|_| format!("Invalid baud rate in config: {:?}", config.baud))?;

    let sdo_timeout_ms: u64 = config
        .sdo_timeout_str
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|&v| v > 0)
        .ok_or_else(|| {
            format!(
                "SDO timeout must be a positive integer (ms), got {:?}",
                config.sdo_timeout_str
            )
        })?;

    let nodes: Vec<(u8, Option<std::path::PathBuf>)> = config
        .nodes
        .iter()
        .filter(|e| !e.id_str.trim().is_empty())
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
                Some(std::path::PathBuf::from(e.eds_path.trim()))
            };
            Ok((id, path))
        })
        .collect::<Result<_, String>>()?;

    let log_path = match config.log_path {
        Some(p) if !p.trim().is_empty() => p,
        _ => "rustycan.jsonl".into(),
    };

    Ok(crate::session::SessionConfig {
        port: config.port.trim().to_string(),
        baud,
        nodes,
        log_path,
        listen_only: config.listen_only,
        text_log: config.text_log,
        sdo_timeout_ms,
        block_initiate_timeout_ms: 1000,
        block_subblock_timeout_ms: 500,
        block_end_timeout_ms: 1000,
        block_size: 64,
        adapter_kind: config.adapter_kind,
        dbc_paths: config
            .dbc_files
            .iter()
            .filter(|e| !e.path.trim().is_empty())
            .map(|e| std::path::PathBuf::from(e.path.trim()))
            .collect(),
        sse_tx,
    })
}
