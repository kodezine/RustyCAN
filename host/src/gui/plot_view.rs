//! Native plot window for live signal visualisation.
//!
//! Opened as a second OS window via [`render`], called each frame from
//! `ctx.show_viewport_immediate` inside `render_monitor`.
//! [`PlotState`] is owned by [`super::MonitorView`] and populated each frame
//! from the CAN event stream before [`eframe::App::update`] completes.
use std::collections::{HashMap, VecDeque};

use egui_plot::{HLine, Legend, Line, Plot, PlotPoints};

use crate::app::CanEvent;
use crate::canopen::pdo::PdoRawValue;

// ─── Constants ────────────────────────────────────────────────────────────────

/// Maximum number of `[t, v]` samples kept per signal.
const MAX_HISTORY_POINTS: usize = 10_000;

/// Number of chart pages shown in the tab strip.
pub const NUM_CHARTS: usize = 8;

// ─── Signal identity ──────────────────────────────────────────────────────────

/// Identifies the CAN source of a plottable signal.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PlotSignalSource {
    /// A PDO signal, uniquely identified by the originating node and COB-ID.
    Pdo { node_id: u8, cob_id: u16 },
    /// A DBC-decoded signal, uniquely identified by the CAN frame ID and
    /// the DBC message name (so ID collisions across buses stay distinct).
    Dbc { can_id: u32, message_name: String },
}

/// A unique key for one plottable signal.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SignalKey {
    pub source: PlotSignalSource,
    /// Signal / object name as decoded from the EDS or DBC.
    pub name: String,
}

impl SignalKey {
    /// Short human-readable label for display inside charts and the picker.
    pub fn label(&self) -> String {
        match &self.source {
            PlotSignalSource::Pdo { node_id, cob_id } => {
                format!("N{node_id}/0x{cob_id:X}/{}", self.name)
            }
            PlotSignalSource::Dbc { message_name, .. } => {
                format!("{message_name}/{}", self.name)
            }
        }
    }
}

// ─── Signal history ───────────────────────────────────────────────────────────

/// Time-series ring buffer for one signal.
pub struct SignalHistory {
    /// Physical unit string (e.g. `"rpm"`, `""`, `"V"`).
    pub unit: String,
    /// Circular buffer of `[unix_seconds, value]` pairs.
    pub points: VecDeque<[f64; 2]>,
    /// Whether this signal is visible in the chart (reserved for per-signal hide toggle).
    #[allow(dead_code)]
    pub visible: bool,
}

impl SignalHistory {
    fn new(unit: String) -> Self {
        Self {
            unit,
            points: VecDeque::with_capacity(MAX_HISTORY_POINTS),
            visible: true,
        }
    }

    fn push(&mut self, t: f64, v: f64) {
        if self.points.len() >= MAX_HISTORY_POINTS {
            self.points.pop_front();
        }
        self.points.push_back([t, v]);
    }
}

// ─── Chart configuration ──────────────────────────────────────────────────────

/// Configuration for one chart page.
pub struct ChartConfig {
    /// Tab label.
    pub title: String,
    /// Signals currently assigned to this chart.
    pub assigned: Vec<SignalKey>,
    /// How many seconds of history are shown (rolling window).
    pub time_window_secs: f64,
    /// Fixed Y range; `None` means auto-fit.
    pub y_range: Option<[f64; 2]>,
    /// Horizontal reference lines.
    pub thresholds: Vec<f64>,
}

impl ChartConfig {
    fn new(index: usize) -> Self {
        Self {
            title: format!("Chart {}", index + 1),
            assigned: Vec::new(),
            time_window_secs: 30.0,
            y_range: None,
            thresholds: Vec::new(),
        }
    }
}

// ─── Plot state ───────────────────────────────────────────────────────────────

/// All plot-related state owned by [`super::MonitorView`].
pub struct PlotState {
    /// All discovered signals and their ring-buffer histories.
    pub registry: HashMap<SignalKey, SignalHistory>,
    /// The eight chart page configurations.
    pub charts: [ChartConfig; NUM_CHARTS],
    /// Index of the currently visible chart tab (0–7).
    pub active_chart: usize,
    /// Whether the signal picker sidebar is open.
    pub picker_open: bool,
    /// Text filter applied to signal names in the picker.
    pub filter_text: String,
    /// When `Some(node_id)`, only PDO signals from that node are shown.
    pub pdo_node_filter: Option<u8>,
}

impl Default for PlotState {
    fn default() -> Self {
        Self {
            registry: HashMap::new(),
            charts: std::array::from_fn(ChartConfig::new),
            active_chart: 0,
            picker_open: false,
            filter_text: String::new(),
            pdo_node_filter: None,
        }
    }
}

// ─── f64 conversion ───────────────────────────────────────────────────────────

/// Extract a plottable `f64` from a [`PdoRawValue`].
/// Returns `None` for non-numeric variants (`Text`, `Bytes`).
pub fn pdo_to_f64(v: &PdoRawValue) -> Option<f64> {
    match v {
        PdoRawValue::Integer(i) => Some(*i as f64),
        PdoRawValue::Unsigned(u) => Some(*u as f64),
        PdoRawValue::Float(f) => Some(*f),
        PdoRawValue::Text(_) | PdoRawValue::Bytes(_) => None,
    }
}

// ─── Event ingestion ──────────────────────────────────────────────────────────

impl PlotState {
    /// Ingest a single decoded CAN event into the signal registry.
    ///
    /// `t_secs` is the Unix timestamp of the sample (seconds since epoch).
    /// Call this **before** passing the event to `apply_event` so the event
    /// is not consumed.
    pub fn feed_event(&mut self, ev: &CanEvent, t_secs: f64) {
        match ev {
            CanEvent::Pdo {
                node_id,
                cob_id,
                values,
            } => {
                for pdo_val in values {
                    if let Some(v) = pdo_to_f64(&pdo_val.value) {
                        let key = SignalKey {
                            source: PlotSignalSource::Pdo {
                                node_id: *node_id,
                                cob_id: *cob_id,
                            },
                            name: pdo_val.signal_name.clone(),
                        };
                        self.registry
                            .entry(key)
                            .or_insert_with(|| SignalHistory::new(String::new()))
                            .push(t_secs, v);
                    }
                }
            }
            CanEvent::DbcSignal(frame) => {
                for sig in &frame.values {
                    let key = SignalKey {
                        source: PlotSignalSource::Dbc {
                            can_id: frame.can_id,
                            message_name: frame.message_name.clone(),
                        },
                        name: sig.signal_name.clone(),
                    };
                    self.registry
                        .entry(key)
                        .or_insert_with(|| SignalHistory::new(sig.unit.clone()))
                        .push(t_secs, sig.physical);
                }
            }
            // NMT, SDO, errors — not plottable.
            _ => {}
        }
    }
}

// ─── Rendering ────────────────────────────────────────────────────────────────

/// Entry point called from `ctx.show_viewport_immediate`.
///
/// Handles the `ViewportClass::EmbeddedWindow` fallback (when the egui backend
/// does not support multiple OS windows) by wrapping content in an `egui::Window`.
pub fn render(
    ui: &mut egui::Ui,
    class: egui::ViewportClass,
    state: &mut PlotState,
    node_labels: &[(u8, String)],
) {
    match class {
        egui::ViewportClass::EmbeddedWindow => {
            egui::Window::new("Plots")
                .resizable(true)
                .show(ui.ctx(), |ui| render_inner(ui, state, node_labels));
        }
        _ => {
            egui::CentralPanel::default()
                .show_inside(ui, |ui| render_inner(ui, state, node_labels));
        }
    }
}

// ─── Inner layout ─────────────────────────────────────────────────────────────

fn render_inner(ui: &mut egui::Ui, state: &mut PlotState, node_labels: &[(u8, String)]) {
    // ── Tab strip ─────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        for i in 0..NUM_CHARTS {
            let label = state.charts[i].title.clone();
            if ui
                .selectable_label(state.active_chart == i, &label)
                .clicked()
            {
                state.active_chart = i;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let picker_label = if state.picker_open {
                "◀ Signals"
            } else {
                "▶ Signals"
            };
            if ui.button(picker_label).clicked() {
                state.picker_open = !state.picker_open;
            }
        });
    });

    ui.separator();

    // ── Time window selector ──────────────────────────────────────────────
    let chart = &mut state.charts[state.active_chart];
    ui.horizontal(|ui| {
        ui.label("Window:");
        for (label, secs) in [
            ("10 s", 10.0),
            ("30 s", 30.0),
            ("1 min", 60.0),
            ("5 min", 300.0),
        ] {
            if ui
                .selectable_label(chart.time_window_secs == secs, label)
                .clicked()
            {
                chart.time_window_secs = secs;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{} signal(s)", chart.assigned.len()))
                    .color(egui::Color32::from_gray(140)),
            );
        });
    });

    ui.separator();

    // ── Split: chart + optional picker sidebar ────────────────────────────
    if state.picker_open {
        // Borrow disjointly: chart from charts, picker from registry.
        egui::Panel::right("signal_picker")
            .resizable(true)
            .default_size(220.0)
            .show_inside(ui, |ui| {
                render_picker(
                    ui,
                    &mut state.charts[state.active_chart],
                    &state.registry,
                    &mut state.filter_text,
                    &mut state.pdo_node_filter,
                    node_labels,
                );
            });
    }

    // Central chart area.
    let active = state.active_chart;
    render_chart(ui, &state.charts[active], &state.registry);
}

// ─── Chart renderer ───────────────────────────────────────────────────────────

fn render_chart(
    ui: &mut egui::Ui,
    chart: &ChartConfig,
    registry: &HashMap<SignalKey, SignalHistory>,
) {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let window_start = now_secs - chart.time_window_secs;

    let mut plot = Plot::new("chart")
        .legend(Legend::default())
        .x_axis_label("time (s ago)")
        .allow_zoom(true)
        .allow_drag(true);

    if let Some([ymin, ymax]) = chart.y_range {
        plot = plot.include_y(ymin).include_y(ymax);
    }

    plot.show(ui, |plot_ui| {
        if chart.assigned.is_empty() {
            return;
        }

        for key in &chart.assigned {
            let Some(hist) = registry.get(key) else {
                continue;
            };

            // Build points within the time window, converting absolute
            // timestamps to "seconds ago" for a natural x-axis.
            let pts: Vec<[f64; 2]> = hist
                .points
                .iter()
                .filter(|p| p[0] >= window_start)
                .map(|p| [p[0] - now_secs, p[1]])
                .collect();

            if pts.is_empty() {
                continue;
            }

            let name = if hist.unit.is_empty() {
                key.label()
            } else {
                format!("{} [{}]", key.label(), hist.unit)
            };

            plot_ui.line(Line::new(name, PlotPoints::new(pts)));
        }

        for &thr in &chart.thresholds {
            plot_ui.hline(HLine::new(format!("threshold {thr:.3}"), thr));
        }
    });
}

// ─── Signal picker ────────────────────────────────────────────────────────────

fn render_picker(
    ui: &mut egui::Ui,
    chart: &mut ChartConfig,
    registry: &HashMap<SignalKey, SignalHistory>,
    filter_text: &mut String,
    pdo_node_filter: &mut Option<u8>,
    node_labels: &[(u8, String)],
) {
    ui.strong("Assign signals");
    ui.separator();

    // ── Search filter ──────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(filter_text)
                .desired_width(f32::INFINITY)
                .hint_text("filter signals…"),
        );
        if !filter_text.is_empty() && ui.small_button("×").clicked() {
            filter_text.clear();
        }
    });

    // ── PDO node selector (only when more than one node is configured) ─────
    if node_labels.len() > 1 {
        let selected_label = pdo_node_filter
            .as_ref()
            .and_then(|id| node_labels.iter().find(|(n, _)| n == id))
            .map(|(_, name)| name.as_str())
            .unwrap_or("All nodes");
        egui::ComboBox::from_id_salt("pdo_node_filter_combo")
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(pdo_node_filter.is_none(), "All nodes")
                    .clicked()
                {
                    *pdo_node_filter = None;
                }
                for (id, name) in node_labels {
                    let label = format!("Node {id}: {name}");
                    if ui
                        .selectable_label(*pdo_node_filter == Some(*id), &label)
                        .clicked()
                    {
                        *pdo_node_filter = Some(*id);
                    }
                }
            });
    }

    if registry.is_empty() {
        ui.label(
            egui::RichText::new("No signals received yet.")
                .italics()
                .color(egui::Color32::from_gray(120)),
        );
        return;
    }

    let filter_lower = filter_text.to_lowercase();

    // ── Assigned signals (with remove button) ────────────────────────────
    if !chart.assigned.is_empty() {
        ui.label(egui::RichText::new("Assigned:").strong());
        let mut to_remove: Option<usize> = None;
        for (idx, key) in chart.assigned.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui
                    .small_button("×")
                    .on_hover_text("Remove from this chart")
                    .clicked()
                {
                    to_remove = Some(idx);
                }
                ui.label(key.label());
            });
        }
        if let Some(idx) = to_remove {
            chart.assigned.remove(idx);
        }
        ui.separator();
    }

    // ── Available signals tree, grouped by source ─────────────────────────
    ui.label(egui::RichText::new("Available:").strong());

    // Collect, filter, and sort registry keys for stable display.
    let mut pdo_keys: Vec<&SignalKey> = Vec::new();
    let mut dbc_keys: Vec<&SignalKey> = Vec::new();
    for key in registry.keys() {
        let label_lower = key.label().to_lowercase();
        if !filter_lower.is_empty() && !label_lower.contains(&filter_lower) {
            continue;
        }
        match &key.source {
            PlotSignalSource::Pdo { node_id, .. } => {
                if pdo_node_filter.map(|f| f == *node_id).unwrap_or(true) {
                    pdo_keys.push(key);
                }
            }
            PlotSignalSource::Dbc { .. } => dbc_keys.push(key),
        }
    }
    pdo_keys.sort_by_key(|k| k.label());
    dbc_keys.sort_by_key(|k| k.label());

    egui::ScrollArea::vertical().show(ui, |ui| {
        if !pdo_keys.is_empty() {
            ui.collapsing("PDO", |ui| {
                for key in &pdo_keys {
                    let assigned = chart.assigned.contains(key);
                    let resp = ui.selectable_label(assigned, key.label());
                    if resp.clicked() {
                        if assigned {
                            chart.assigned.retain(|k| k != *key);
                        } else {
                            chart.assigned.push((*key).clone());
                        }
                    }
                }
            });
        }

        if !dbc_keys.is_empty() {
            ui.collapsing("DBC", |ui| {
                for key in &dbc_keys {
                    let assigned = chart.assigned.contains(key);
                    let resp = ui.selectable_label(assigned, key.label());
                    if resp.clicked() {
                        if assigned {
                            chart.assigned.retain(|k| k != *key);
                        } else {
                            chart.assigned.push((*key).clone());
                        }
                    }
                }
            });
        }

        if pdo_keys.is_empty() && dbc_keys.is_empty() && !filter_lower.is_empty() {
            ui.label(
                egui::RichText::new("No matching signals.")
                    .italics()
                    .color(egui::Color32::from_gray(120)),
            );
        }
    });
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pdo_to_f64_integer() {
        assert_eq!(pdo_to_f64(&PdoRawValue::Integer(8)), Some(8.0));
        assert_eq!(pdo_to_f64(&PdoRawValue::Integer(-3)), Some(-3.0));
    }

    #[test]
    fn test_pdo_to_f64_unsigned() {
        assert_eq!(pdo_to_f64(&PdoRawValue::Unsigned(255)), Some(255.0));
    }

    #[test]
    fn test_pdo_to_f64_float() {
        let v = pdo_to_f64(&PdoRawValue::Float(1.5)).unwrap();
        assert!((v - 1.5).abs() < 1e-9);
    }

    #[test]
    fn test_pdo_to_f64_non_numeric() {
        assert_eq!(pdo_to_f64(&PdoRawValue::Text("x".into())), None);
        assert_eq!(pdo_to_f64(&PdoRawValue::Bytes(vec![0x01])), None);
    }

    #[test]
    fn test_ring_buffer_trim() {
        let mut hist = SignalHistory::new(String::new());
        for i in 0..=(MAX_HISTORY_POINTS) {
            hist.push(i as f64, i as f64);
        }
        assert_eq!(hist.points.len(), MAX_HISTORY_POINTS);
    }

    #[test]
    fn test_chart_config_default_title() {
        let cfg = ChartConfig::new(0);
        assert_eq!(cfg.title, "Chart 1");
        let cfg7 = ChartConfig::new(7);
        assert_eq!(cfg7.title, "Chart 8");
    }

    #[test]
    fn test_plot_state_default() {
        let state = PlotState::default();
        assert_eq!(state.active_chart, 0);
        assert!(!state.picker_open);
        assert_eq!(state.charts.len(), NUM_CHARTS);
        assert!(state.filter_text.is_empty());
        assert_eq!(state.pdo_node_filter, None);
    }

    #[test]
    fn filter_narrows_dbc_keys() {
        // Build a small registry with two DBC keys.
        let mut registry: HashMap<SignalKey, SignalHistory> = HashMap::new();
        let key_speed = SignalKey {
            source: PlotSignalSource::Dbc {
                can_id: 0,
                message_name: "Engine".to_string(),
            },
            name: "EngineSpeed".to_string(),
        };
        let key_temp = SignalKey {
            source: PlotSignalSource::Dbc {
                can_id: 0,
                message_name: "Engine".to_string(),
            },
            name: "CoolantTemp".to_string(),
        };
        registry.insert(key_speed.clone(), SignalHistory::new(String::new()));
        registry.insert(key_temp.clone(), SignalHistory::new(String::new()));

        let filter = "speed";
        let filter_lower = filter.to_lowercase();
        let mut dbc_keys: Vec<&SignalKey> = registry
            .keys()
            .filter(|k| k.label().to_lowercase().contains(&filter_lower))
            .collect();
        dbc_keys.sort_by_key(|k| k.label());
        assert_eq!(dbc_keys.len(), 1);
        assert_eq!(dbc_keys[0].name, "EngineSpeed");
    }

    #[test]
    fn pdo_node_filter_excludes_other_nodes() {
        let mut registry: HashMap<SignalKey, SignalHistory> = HashMap::new();
        let key1 = SignalKey {
            source: PlotSignalSource::Pdo {
                node_id: 1,
                cob_id: 0x181,
            },
            name: "sig".to_string(),
        };
        let key2 = SignalKey {
            source: PlotSignalSource::Pdo {
                node_id: 2,
                cob_id: 0x281,
            },
            name: "sig".to_string(),
        };
        registry.insert(key1.clone(), SignalHistory::new(String::new()));
        registry.insert(key2.clone(), SignalHistory::new(String::new()));

        let node_filter: Option<u8> = Some(1);
        let pdo_keys: Vec<&SignalKey> = registry
            .keys()
            .filter(|k| {
                if let PlotSignalSource::Pdo { node_id, .. } = &k.source {
                    node_filter.map(|f| f == *node_id).unwrap_or(true)
                } else {
                    false
                }
            })
            .collect();
        assert_eq!(pdo_keys.len(), 1);
        assert!(matches!(
            &pdo_keys[0].source,
            PlotSignalSource::Pdo { node_id: 1, .. }
        ));
    }

    #[test]
    fn test_feed_event_pdo() {
        use crate::canopen::pdo::PdoValue;

        let mut state = PlotState::default();
        let ev = CanEvent::Pdo {
            node_id: 1,
            cob_id: 0x181,
            values: vec![PdoValue {
                signal_name: "speed".to_string(),
                value: PdoRawValue::Unsigned(100),
            }],
        };
        state.feed_event(&ev, 1000.0);

        let key = SignalKey {
            source: PlotSignalSource::Pdo {
                node_id: 1,
                cob_id: 0x181,
            },
            name: "speed".to_string(),
        };
        let hist = state.registry.get(&key).unwrap();
        assert_eq!(hist.points.len(), 1);
        assert_eq!(hist.points[0], [1000.0, 100.0]);
    }
}
