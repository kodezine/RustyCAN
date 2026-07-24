//! egui view for the RustyCAN sniffer model.
//!
//! These are small, composable render functions over a
//! [`sniffer_core::SnifferModel`] so the same UI can be dropped into the dev
//! harness (JSONL replay) and the live RustyCAN app (channel-fed) without
//! change. Panel layout is left to the caller.

use egui::Color32;
use sniffer_core::{
    fmt_can_id, parse_byte_fields, parse_hex_u32, short_type, PeriodicMsg, Replay, SnifferBackend,
    SnifferModel, HIGHLIGHT_SECS,
};

pub const ACCENT: Color32 = Color32::from_rgb(0x4d, 0x9d, 0xff);
pub const TX_COLOR: Color32 = Color32::from_rgb(0x3d, 0xd6, 0x8c);
const BIT_ON: Color32 = Color32::from_rgb(0x3d, 0xd6, 0x8c);

// Fixed table column widths (no horizontal scrolling needed).
// One uniform monospace font across the whole table.
const FONT: f32 = 12.0;
const HFONT: f32 = 12.0; // header font (same size as body)
const COL_TIME: f32 = 90.0; // fits "HH:MM:SS.mmm"
const COL_ID: f32 = 88.0; // fits extended "0x1FFFFFFF" (10 chars)
const COL_TYPE: f32 = 42.0;
const COL_DLC: f32 = 20.0;
const COL_BYTE: f32 = 22.0;
const COL_COUNT: f32 = 70.0;
const ROW_H: f32 = 16.0;

/// Text-input buffers for the transmit compose row (UI-only state).
pub struct SnifferUiState {
    pub tx_id: String,
    pub tx_bytes: [String; 8],
    pub tx_period_ms: f64,
}

impl Default for SnifferUiState {
    fn default() -> Self {
        Self {
            tx_id: "600".to_string(),
            tx_bytes: [
                "40".into(),
                "00".into(),
                "10".into(),
                "00".into(),
                "".into(),
                "".into(),
                "".into(),
                "".into(),
            ],
            tx_period_ms: 100.0,
        }
    }
}

/// Apply the sniffer's dark, compact style to the context (optional).
pub fn apply_style(ctx: &egui::Context) {
    ctx.global_style_mut(|style| {
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = Color32::from_rgb(0x1c, 0x1f, 0x26);
        style.visuals.window_fill = Color32::from_rgb(0x1c, 0x1f, 0x26);
        style.visuals.extreme_bg_color = Color32::from_rgb(0x12, 0x14, 0x18);
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(6.0, 2.0);
        style.spacing.interact_size.y = 20.0;
        style.spacing.scroll.floating = false;
    });
}

fn byte_highlight(changed_at: f64, now: f64) -> Option<Color32> {
    let age = now - changed_at;
    if !(0.0..=HIGHLIGHT_SECS).contains(&age) {
        return None;
    }
    let t = 1.0 - (age / HIGHLIGHT_SECS) as f32;
    let a = (t * 190.0) as u8;
    Some(Color32::from_rgba_unmultiplied(0xE0, 0x3b, 0x3b, a))
}

fn hex_of(b: Option<&u8>) -> String {
    b.map(|v| format!("{v:02X}")).unwrap_or_default()
}

/// Fixed 8-slot hex payload for aligned display, e.g. `40 00 10 00 -- -- -- --`.
/// Missing bytes are rendered as `--` so every row's trailing columns align.
fn hex8(bytes: &[u8]) -> String {
    (0..8)
        .map(|i| {
            bytes
                .get(i)
                .map(|b| format!("{b:02X}"))
                .unwrap_or_else(|| "--".to_string())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Force every text style in this `Ui` subtree to the compact sniffer font size,
/// so labels, values, headers, buttons and mono text all match. Call once on the
/// parent `Ui` before rendering the sniffer widgets/panels.
pub fn apply_compact_text(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    for ts in [
        egui::TextStyle::Heading,
        egui::TextStyle::Body,
        egui::TextStyle::Button,
        egui::TextStyle::Small,
        egui::TextStyle::Monospace,
    ] {
        if let Some(font_id) = style.text_styles.get_mut(&ts) {
            font_id.size = FONT;
        }
    }
}

/// Filter row: ID substring filter + hide-heartbeats toggle.
pub fn filter_bar(ui: &mut egui::Ui, model: &mut SnifferModel) {
    apply_compact_text(ui);
    ui.horizontal(|ui| {
        ui.label("Filter ID");
        ui.add(egui::TextEdit::singleline(&mut model.filter).desired_width(80.0));
        ui.checkbox(
            &mut model.hide_heartbeat,
            "Hide heartbeats (0x700\u{2013}0x77F)",
        );
    });
}

/// Replay transport: play/pause, reset, speed, scrub. Optional (live app omits).
pub fn replay_controls(ui: &mut egui::Ui, replay: &mut Replay, model: &mut SnifferModel, now: f64) {
    ui.horizontal(|ui| {
        let play_label = if replay.playing {
            "\u{23F8} Pause"
        } else {
            "\u{25B6} Play"
        };
        if ui.button(play_label).clicked() {
            let p = replay.playing;
            replay.set_playing(!p, model, now);
        }
        if ui.button("\u{23EE} Reset").clicked() {
            replay.reset(model, now);
        }
        ui.label("Speed");
        ui.add(
            egui::Slider::new(&mut replay.speed, 0.25..=100.0)
                .logarithmic(true)
                .suffix("x"),
        );
        ui.separator();
        let total = replay.len();
        let mut pos = replay.pos;
        ui.label(format!("{pos}/{total}"));
        if ui
            .add(egui::Slider::new(&mut pos, 0..=total).show_value(false))
            .changed()
        {
            replay.rebuild_to(model, pos, now);
        }
    });
}

/// The aggregated packet table (fixed-width columns, click a byte to select).
pub fn table(ui: &mut egui::Ui, model: &mut SnifferModel, now: f64) {
    apply_compact_text(ui);
    let sel_id = model.selected_id;
    let sel_byte = model.selected_byte;
    let mut clicked: Option<(u32, usize)> = None;

    // Reserve a gutter so the vertical scrollbar doesn't overlap the Count column.
    ui.style_mut().spacing.scroll.floating = false;

    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("sniffer_grid")
            .striped(true)
            .num_columns(13)
            .min_col_width(0.0) // honor our fixed add_sized widths (default floor is ~40px)
            .spacing([5.0, 2.0])
            .show(ui, |ui| {
                let hdr = |ui: &mut egui::Ui, t: &str, w: f32| {
                    ui.add_sized(
                        [w, ROW_H],
                        egui::Label::new(egui::RichText::new(t).monospace().weak().size(HFONT))
                            .truncate(),
                    );
                };
                hdr(ui, "Time", COL_TIME);
                hdr(ui, "ID", COL_ID);
                hdr(ui, "Type", COL_TYPE);
                hdr(ui, "DLC", COL_DLC);
                for b in 0..8 {
                    hdr(ui, &format!("B{b}"), COL_BYTE);
                }
                ui.allocate_ui_with_layout(
                    egui::vec2(COL_COUNT, ROW_H),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.label(egui::RichText::new("Count").monospace().weak().size(HFONT));
                    },
                );
                ui.end_row();

                for (&id, row) in model.rows.iter() {
                    if !model.row_visible(id) {
                        continue;
                    }
                    ui.add_sized(
                        [COL_TIME, ROW_H],
                        egui::Label::new(
                            egui::RichText::new(&row.last_disp).monospace().size(FONT),
                        )
                        .truncate(),
                    );

                    let id_txt = egui::RichText::new(fmt_can_id(id)).monospace().size(FONT);
                    let id_txt = if row.is_tx {
                        id_txt.color(TX_COLOR)
                    } else {
                        id_txt
                    };
                    if ui
                        .add_sized(
                            [COL_ID, ROW_H],
                            egui::Label::new(id_txt).sense(egui::Sense::click()),
                        )
                        .clicked()
                    {
                        clicked = Some((id, 0));
                    }

                    ui.add_sized(
                        [COL_TYPE, ROW_H],
                        egui::Label::new(
                            egui::RichText::new(short_type(&row.typ))
                                .monospace()
                                .size(FONT),
                        )
                        .truncate(),
                    );
                    ui.add_sized(
                        [COL_DLC, ROW_H],
                        egui::Label::new(
                            egui::RichText::new(format!("{}", row.bytes.len()))
                                .monospace()
                                .size(FONT),
                        ),
                    );

                    for i in 0..8 {
                        if i < row.bytes.len() {
                            let selected = sel_id == Some(id) && sel_byte == i;
                            let mut txt = egui::RichText::new(hex_of(row.bytes.get(i)))
                                .monospace()
                                .size(FONT);
                            if selected {
                                txt = txt.background_color(ACCENT).color(Color32::BLACK);
                            } else if let Some(c) = byte_highlight(row.changed[i], now) {
                                txt = txt.background_color(c);
                            }
                            if ui
                                .add_sized(
                                    [COL_BYTE, ROW_H],
                                    egui::Label::new(txt).sense(egui::Sense::click()),
                                )
                                .clicked()
                            {
                                clicked = Some((id, i));
                            }
                        } else {
                            ui.add_sized([COL_BYTE, ROW_H], egui::Label::new(""));
                        }
                    }

                    ui.allocate_ui_with_layout(
                        egui::vec2(COL_COUNT, ROW_H),
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.add(egui::Label::new(
                                egui::RichText::new(format!("{}", row.count))
                                    .weak()
                                    .monospace()
                                    .size(FONT),
                            ));
                        },
                    );
                    ui.end_row();
                }
            });
    });

    if let Some((id, i)) = clicked {
        model.selected_id = Some(id);
        model.selected_byte = i;
    }
}

/// Control counters + decoded-value inspector for the selected cell.
pub fn inspector(ui: &mut egui::Ui, model: &mut SnifferModel, replay: Option<&Replay>) {
    apply_compact_text(ui);
    ui.add_space(6.0);
    ui.label(egui::RichText::new("CONTROL").weak().small());
    ui.separator();
    egui::Grid::new("control_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            if let Some(r) = replay {
                ui.label("Frames");
                ui.monospace(format!("{} / {}", r.pos, r.len()));
                ui.end_row();
            }
            ui.label("Unique IDs");
            ui.monospace(format!("{}", model.rows.len()));
            ui.end_row();
            ui.label("Periodic");
            let active = model.periodics.iter().filter(|m| m.enabled).count();
            ui.monospace(format!("{active} active / {}", model.periodics.len()));
            ui.end_row();
        });

    ui.add_space(12.0);
    ui.label(egui::RichText::new("DECODED VALUES").weak().small());
    ui.separator();

    let Some(d) = model.decoded() else {
        ui.label(egui::RichText::new("Click a byte in the table.").weak());
        return;
    };

    egui::Grid::new("decode_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.label("Selected");
            ui.monospace(format!("{}  B{}", fmt_can_id(d.id), d.byte_index));
            ui.end_row();
            ui.label("Binary byte");
            ui.monospace(format!("{:08b}", d.byte));
            ui.end_row();
            ui.label("Decimal byte");
            ui.monospace(format!("{}", d.byte));
            ui.end_row();
            ui.label("Hex byte");
            ui.monospace(format!("0x{:02X}", d.byte));
            ui.end_row();
            ui.label("Decimal word");
            ui.monospace(format!("{}", d.word));
            ui.end_row();
        });

    ui.checkbox(&mut model.decode_big_endian, "Byte order: big-endian");
    ui.horizontal(|ui| {
        ui.label("Word multiplier");
        ui.add(
            egui::DragValue::new(&mut model.word_multiplier)
                .speed(0.001)
                .range(0.0..=1_000_000.0),
        );
    });
    ui.horizontal(|ui| {
        ui.label("Result");
        ui.monospace(format!("{:.6}", d.word as f64 * model.word_multiplier));
    });
    let _ = BIT_ON; // reserved for future LED view
}

/// Transmit compose row + periodic-message list + status line.
pub fn send_bar(
    ui: &mut egui::Ui,
    model: &mut SnifferModel,
    ui_state: &mut SnifferUiState,
    backend: &mut dyn SnifferBackend,
    now: f64,
) {
    apply_compact_text(ui);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Send").color(TX_COLOR).strong());
        ui.label("ID");
        ui.add(egui::TextEdit::singleline(&mut ui_state.tx_id).desired_width(56.0));
        ui.label("data");
        for i in 0..8 {
            ui.add(
                egui::TextEdit::singleline(&mut ui_state.tx_bytes[i])
                    .desired_width(28.0)
                    .font(egui::TextStyle::Monospace),
            );
        }
        if ui.button("\u{1F4E4} Send").clicked() {
            match compose(ui_state) {
                Ok((id, bytes)) => model.send_manual(id, &bytes, now, backend),
                Err(e) => model.status = Some(e),
            }
        }
        ui.separator();
        ui.add(
            egui::DragValue::new(&mut ui_state.tx_period_ms)
                .speed(1.0)
                .range(1.0..=10_000.0)
                .suffix(" ms"),
        );
        if ui.button("\u{2795} Add periodic").clicked() {
            match compose(ui_state) {
                Ok((id, bytes)) => model.add_periodic(id, bytes, ui_state.tx_period_ms),
                Err(e) => model.status = Some(e),
            }
        }
    });

    if !model.periodics.is_empty() {
        ui.separator();
        ui.label(egui::RichText::new("Periodic Messages").weak());
        let mut remove: Option<usize> = None;
        for i in 0..model.periodics.len() {
            ui.horizontal(|ui| {
                let m: &mut PeriodicMsg = &mut model.periodics[i];
                ui.checkbox(&mut m.enabled, "");
                ui.monospace(fmt_can_id(m.id));
                ui.monospace(format!("[{}]", hex8(&m.bytes)));
                ui.add(
                    egui::DragValue::new(&mut m.period_ms)
                        .speed(1.0)
                        .range(1.0..=10_000.0)
                        .suffix(" ms"),
                );
                ui.label(
                    egui::RichText::new(format!("{:.1} Hz", 1000.0 / m.period_ms.max(1.0)))
                        .weak()
                        .small(),
                );
                ui.weak(format!("\u{2191}{}", m.count));
                let del = egui::Button::new(
                    egui::RichText::new("\u{1F5D1}")
                        .color(egui::Color32::from_rgb(0xE0, 0x3b, 0x3b)),
                );
                if ui.add(del).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            model.remove_periodic(i);
        }
    }

    ui.horizontal(|ui| {
        ui.colored_label(ACCENT, "\u{2699}");
        match &model.status {
            Some(s) => ui.monospace(s),
            None => ui.label(egui::RichText::new("Ready").weak()),
        };
    });
}

fn compose(ui_state: &SnifferUiState) -> Result<(u32, Vec<u8>), String> {
    let id = parse_hex_u32(&ui_state.tx_id).ok_or_else(|| "Invalid TX CAN ID".to_string())?;
    let bytes = parse_byte_fields(&ui_state.tx_bytes)?;
    Ok((id, bytes))
}
