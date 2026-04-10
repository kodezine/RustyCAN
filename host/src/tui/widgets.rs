//! Ratatui widget render functions for the RustyCAN TUI.
//!
//! Each function takes a [`ratatui::Frame`], a reference to the shared
//! [`AppState`], and the target [`ratatui::layout::Rect`] to draw into.
//! All functions are pure: they only read state and issue draw calls.

use std::collections::VecDeque;

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Row, Table},
    Frame,
};

use crate::app::AppState;
use crate::canopen::nmt::NmtState;

use super::TuiMode;

// ─── NMT panel ───────────────────────────────────────────────────────────────

/// Render the NMT node status table.
///
/// Shows one row per configured node with its node-ID, EDS label, and current
/// NMT state.  Nodes are sorted by ID for stable ordering.
pub fn render_nmt_panel(f: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default()
        .title(" NMT Status ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let header = Row::new(vec!["Node", "Label", "State", "Heartbeat"])
        .style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Yellow),
        )
        .bottom_margin(0);

    let mut nodes: Vec<_> = state.node_map.iter().collect();
    nodes.sort_by_key(|(id, _)| *id);

    let rows: Vec<Row> = nodes
        .iter()
        .map(|(id, (label, nmt_state, ts))| {
            let state_str = nmt_state.to_string();
            let state_color = nmt_state_color(nmt_state);

            let hb = ts
                .as_ref()
                .and_then(|(_, period)| period.as_ref())
                .map(|d| format!("{:.0}ms", d.as_millis()))
                .unwrap_or_else(|| "-".into());

            Row::new(vec![format!("{id}"), label.clone(), state_str, hb])
                .style(Style::default().fg(state_color))
        })
        .collect();

    let widths = [
        ratatui::layout::Constraint::Length(6),
        ratatui::layout::Constraint::Percentage(40),
        ratatui::layout::Constraint::Percentage(40),
        ratatui::layout::Constraint::Length(10),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(1);

    f.render_widget(table, area);
}

fn nmt_state_color(state: &NmtState) -> Color {
    match state {
        NmtState::Operational => Color::Green,
        NmtState::PreOperational => Color::Yellow,
        NmtState::Stopped => Color::Red,
        NmtState::Bootup => Color::Cyan,
        NmtState::Unknown(_) => Color::DarkGray,
    }
}

// ─── PDO panel ───────────────────────────────────────────────────────────────

/// Render the live PDO signal values panel.
///
/// Groups signals by `(node_id, cob_id)` and shows the most-recent decoded
/// values.  Entries are sorted by node-ID then COB-ID for stable ordering.
pub fn render_pdo_panel(f: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default()
        .title(" PDO Live Values ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));

    let mut entries: Vec<_> = state.pdo_values.iter().collect();
    entries.sort_by_key(|((node, cob), _)| (*node, *cob));

    let items: Vec<ListItem> = entries
        .iter()
        .flat_map(|((node_id, cob_id), (values, _ts, period))| {
            let hb = period
                .as_ref()
                .map(|d| format!("  {:.0}ms", d.as_millis()))
                .unwrap_or_default();

            let header_line = Line::from(vec![Span::styled(
                format!("Node {node_id}  COB 0x{cob_id:03X}{hb}"),
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            )]);

            let mut lines: Vec<Line> = vec![header_line];
            for pv in values {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{} = {}", pv.signal_name, pv.value),
                        Style::default().fg(Color::White),
                    ),
                ]));
            }
            lines.into_iter().map(ListItem::new).collect::<Vec<_>>()
        })
        .collect();

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

// ─── SDO log ─────────────────────────────────────────────────────────────────

/// Render the SDO transaction log (most-recent entries at the bottom).
///
/// Shows timestamp, node-ID, direction, index/subindex, signal name, and
/// decoded value (or abort code on error).
pub fn render_sdo_log(f: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default()
        .title(" SDO Log ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Magenta));

    let items: Vec<ListItem> = state
        .sdo_log
        .iter()
        .map(|entry| {
            let dir = match entry.direction {
                crate::canopen::sdo::SdoDirection::Read => "R",
                crate::canopen::sdo::SdoDirection::Write => "W",
            };
            let value_str = if let Some(v) = &entry.value {
                format!("{v}")
            } else if let Some(code) = entry.abort_code {
                format!("ABORT 0x{code:08X}")
            } else {
                "pending".into()
            };

            let ts_str = entry.ts.format("%H:%M:%S%.3f").to_string();
            let text = format!(
                "[{ts_str}] N{} {dir} {:04X}:{:02X} {} = {}",
                entry.node_id, entry.index, entry.subindex, entry.name, value_str
            );

            let color = if entry.abort_code.is_some() {
                Color::Red
            } else {
                Color::White
            };
            ListItem::new(text).style(Style::default().fg(color))
        })
        .collect();

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

// ─── Stats bar ───────────────────────────────────────────────────────────────

/// Render the one-row stats bar showing FPS, bus load, and log path.
pub fn render_stats_bar(f: &mut Frame, state: &AppState, area: Rect) {
    let bus_load_color = if state.bus_load >= 80.0 {
        Color::Red
    } else if state.bus_load >= 50.0 {
        Color::Yellow
    } else {
        Color::Green
    };

    let line = Line::from(vec![
        Span::styled(
            format!(" {:.0} fps", state.fps),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw("  "),
        Span::styled(
            format!("Bus {:.1}%", state.bus_load),
            Style::default().fg(bus_load_color),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{} frames", state.total_frames),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("  "),
        Span::styled(
            format!("log: {}", state.log_path),
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    let paragraph = Paragraph::new(line).style(Style::default().bg(Color::Reset));
    f.render_widget(paragraph, area);
}

// ─── Event log panel ─────────────────────────────────────────────────────────

/// Render the scrollable plain-text event log panel.
///
/// Displays the most-recent `log_entries` one line per event.  Newer events
/// appear at the bottom so the panel behaves like a terminal tail.
pub fn render_log_panel(f: &mut Frame, log_entries: &VecDeque<String>, area: Rect) {
    let block = Block::default()
        .title(" Event Log  (press L to hide) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner_height = area.height.saturating_sub(2) as usize;
    let items: Vec<ListItem> = log_entries
        .iter()
        .rev()
        .take(inner_height)
        .rev()
        .map(|line| ListItem::new(line.as_str()).style(Style::default().fg(Color::DarkGray)))
        .collect();

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

// ─── Command bar ─────────────────────────────────────────────────────────────

/// Render the bottom command-hint bar or active input line.
///
/// In [`TuiMode::Normal`] this shows a one-line key-binding reference.
/// In [`TuiMode::Input`] this shows a prompt with the buffer being typed.
pub fn render_command_bar(f: &mut Frame, mode: &TuiMode, area: Rect) {
    let line = match mode {
        TuiMode::Normal => Line::from(vec![
            Span::styled(
                " [n]",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" NMT  "),
            Span::styled(
                "[s]",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" SDO read  "),
            Span::styled(
                "[w]",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" SDO write  "),
            Span::styled(
                "[L]",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" log  "),
            Span::styled(
                "[q]",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" quit"),
        ]),
        TuiMode::Input { kind, buf } => {
            let prompt = match kind {
                super::InputKind::Nmt => "NMT  <node> <command>  e.g. `1 start` → ",
                super::InputKind::SdoRead => {
                    "SDO read  <node> <index_hex> <sub>  e.g. `1 1000 0` → "
                }
                super::InputKind::SdoWrite => {
                    "SDO write  <node> <index_hex> <sub> <hex_value>  e.g. `1 6040 0 0006` → "
                }
            };
            Line::from(vec![
                Span::styled(prompt, Style::default().fg(Color::Cyan)),
                Span::styled(
                    buf.as_str(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "_",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::SLOW_BLINK),
                ),
            ])
        }
    };

    let paragraph = Paragraph::new(line).style(Style::default().bg(Color::Reset));
    f.render_widget(paragraph, area);
}
