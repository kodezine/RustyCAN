/// egui + eframe front-end for RustyCAN.
///
/// Two screens:
///   1. `Connect`  — configure port / baud / nodes / log file, then click Connect
///   2. `Monitor`  — live NMT / PDO / SDO panels with a Disconnect button
///
/// `AppState` and `CanEvent` live in `crate::app` and are UI-independent.
/// The CAN session lifecycle (EDS loading, thread spawning) lives in `crate::session`.
use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui;
use rfd::FileDialog;

use crate::app::{apply_event, AppState, CanEvent};
use crate::canopen::nmt::NmtState;
use crate::canopen::sdo::SdoDirection;
use crate::session::{self, SessionConfig};

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

#[derive(Clone)]
struct ConnectForm {
    port: String,
    baud: String,
    nodes: Vec<NodeEntry>,
    log_path: String,
    /// Non-None when the last connect attempt failed.
    error: Option<String>,
}

#[derive(Clone, Default)]
struct NodeEntry {
    id_str: String,
    eds_path: String,
}

impl Default for ConnectForm {
    fn default() -> Self {
        ConnectForm {
            port: "1".into(),
            baud: "250000".into(),
            nodes: vec![NodeEntry::default()],
            log_path: "rustycan.jsonl".into(),
            error: None,
        }
    }
}

impl ConnectForm {
    /// Validate the form and start a session.
    fn try_connect(&self) -> Result<MonitorView, String> {
        if self.nodes.is_empty() {
            return Err("Add at least one node.".into());
        }

        let baud: u32 = self
            .baud
            .trim()
            .parse()
            .map_err(|_| format!("Invalid baud rate: {:?}", self.baud))?;

        let nodes: Vec<(u8, PathBuf)> = self
            .nodes
            .iter()
            .map(|e| {
                let id: u8 = e
                    .id_str
                    .trim()
                    .parse()
                    .map_err(|_| format!("Invalid node ID: {:?}", e.id_str))?;
                if e.eds_path.trim().is_empty() {
                    return Err(format!("Node {id}: EDS path is empty."));
                }
                Ok((id, PathBuf::from(e.eds_path.trim())))
            })
            .collect::<Result<_, String>>()?;

        let config = SessionConfig {
            port: self.port.trim().to_string(),
            baud,
            nodes,
            log_path: self.log_path.trim().to_string(),
        };

        let (rx, node_labels) = session::start(config)?;

        let mut state = AppState::new(self.log_path.trim().to_string());
        state.init_nodes(&node_labels);

        // Save a clean copy of the form (no error) to restore on disconnect.
        let mut saved_form = self.clone();
        saved_form.error = None;

        Ok(MonitorView {
            rx,
            state,
            form: saved_form,
            disconnected: false,
        })
    }
}

const BAUD_OPTIONS: &[&str] = &["125000", "250000", "500000", "1000000"];

fn render_connect(ctx: &egui::Context, form: &mut ConnectForm) -> Option<Screen> {
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
                            ui.add(
                                egui::TextEdit::singleline(&mut form.port)
                                    .desired_width(80.0)
                                    .hint_text("1"),
                            );
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
                                        .hint_text("e.g. 1"),
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

            if ui.button("  Connect  ").clicked() {
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
    state: AppState,
    /// Saved form — restored when the user clicks Disconnect.
    form: ConnectForm,
    disconnected: bool,
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
            nmt_section(ui, &view.state);
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

fn nmt_section(ui: &mut egui::Ui, state: &AppState) {
    egui::CollapsingHeader::new(egui::RichText::new("NMT Status").strong())
        .default_open(true)
        .show(ui, |ui| {
            egui::Grid::new("nmt_grid")
                .striped(true)
                .min_col_width(80.0)
                .show(ui, |ui| {
                    ui.strong("Node");
                    ui.strong("EDS");
                    ui.strong("State");
                    ui.strong("Last seen");
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
                        ui.end_row();
                    }

                    if state.node_map.is_empty() {
                        ui.label("No nodes configured.");
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
