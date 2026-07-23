//! Dev harness for the sniffer, driven by RustyCAN JSONL logs.
//!
//! This is the "dry sandbox" that used to live in the separate `egui_sandbox`
//! folder. It exercises `sniffer-core` + `sniffer-egui` against recorded logs
//! so UI/UX can be iterated without hardware, then integrated unchanged.
//!
//! Run from the RustyCAN repo root (where the *.jsonl logs live):
//!   cargo run -p sniffer-egui --example dev_sniffer
//! Or point it elsewhere:
//!   SNIFFER_LOG_DIR=/path/to/logs cargo run -p sniffer-egui --example dev_sniffer

use eframe::egui;
use sniffer_core::{jsonl, NullBackend, Replay, SnifferModel};
use sniffer_egui as view;

const MAX_FRAMES: usize = 100_000;

struct DevApp {
    files: Vec<(String, String)>, // (name, full path)
    file_idx: usize,
    meta: String,
    model: SnifferModel,
    replay: Replay,
    ui_state: view::SnifferUiState,
    backend: NullBackend,
}

impl DevApp {
    fn new() -> Self {
        let files = discover_logs();
        let mut app = Self {
            files,
            file_idx: 0,
            meta: String::new(),
            model: SnifferModel::new(),
            replay: Replay::default(),
            ui_state: view::SnifferUiState::default(),
            backend: NullBackend,
        };
        app.load_selected();
        app
    }

    fn load_selected(&mut self) {
        self.model = SnifferModel::new();
        if let Some((_, path)) = self.files.get(self.file_idx).cloned() {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let loaded = jsonl::parse_document(&text, MAX_FRAMES);
            self.meta = loaded.meta;
            self.replay = Replay::new(loaded.frames);
        } else {
            self.meta = "No .jsonl logs found".to_string();
            self.replay = Replay::default();
        }
    }
}

impl eframe::App for DevApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = ui.input(|i| i.time);

        let playing = self.replay.advance(&mut self.model, now);
        let periodic = self.model.pump_periodics(now, &mut self.backend);
        if playing || periodic || self.model.animating(now) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(16));
        }

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading(egui::RichText::new("CAN Sniffer (dev)").color(view::ACCENT));
                ui.separator();
                let current = self
                    .files
                    .get(self.file_idx)
                    .map(|(n, _)| n.clone())
                    .unwrap_or_else(|| "<no logs>".to_string());
                let mut new_idx = self.file_idx;
                egui::ComboBox::from_id_salt("logfile")
                    .selected_text(current)
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        for (i, (name, _)) in self.files.iter().enumerate() {
                            ui.selectable_value(&mut new_idx, i, name);
                        }
                    });
                if new_idx != self.file_idx {
                    self.file_idx = new_idx;
                    self.load_selected();
                }
                if !self.meta.is_empty() {
                    ui.label(egui::RichText::new(&self.meta).weak());
                }
            });
            view::replay_controls(ui, &mut self.replay, &mut self.model, now);
            view::filter_bar(ui, &mut self.model);
            ui.add_space(4.0);
        });

        egui::Panel::right("inspector")
            .resizable(false)
            .exact_size(290.0)
            .show(ui, |ui| {
                view::inspector(ui, &mut self.model, Some(&self.replay));
            });

        egui::Panel::bottom("txbar").show(ui, |ui| {
            ui.add_space(4.0);
            view::send_bar(
                ui,
                &mut self.model,
                &mut self.ui_state,
                &mut self.backend,
                now,
            );
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            view::table(ui, &mut self.model, now);
        });
    }
}

fn discover_logs() -> Vec<(String, String)> {
    let dir = std::env::var("SNIFFER_LOG_DIR").unwrap_or_else(|_| ".".to_string());
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    out.push((name.to_string(), path.to_string_lossy().to_string()));
                }
            }
        }
    }
    out.sort();
    out
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1120.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        "CAN Sniffer (dev harness)",
        options,
        Box::new(|cc| {
            view::apply_style(&cc.egui_ctx);
            Ok(Box::new(DevApp::new()))
        }),
    )
}
