use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

use super::AppState;
use crate::canopen::nmt::NmtState;

// ─── NMT status panel ────────────────────────────────────────────────────────

/// Render the NMT status panel showing per-node state.
pub fn render_nmt_panel(f: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default()
        .title(" NMT Status ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    // Sort by node_id for stable display.
    let mut sorted_nodes: Vec<_> = state.node_map.keys().copied().collect();
    sorted_nodes.sort();
    let rows: Vec<Row> = sorted_nodes
        .iter()
        .map(|id| {
            let (eds_name, nmt_state, last_seen) = &state.node_map[id];
            let age = last_seen
                .map(|t| {
                    let secs = t.elapsed().as_secs_f64();
                    if secs < 60.0 {
                        format!("{secs:.1}s ago")
                    } else {
                        format!("{:.0}m ago", secs / 60.0)
                    }
                })
                .unwrap_or_else(|| "—".into());
            let (state_str, state_color) = nmt_state_style(nmt_state);
            Row::new(vec![
                Cell::from(format!(" {id:3}")),
                Cell::from(eds_name.as_str()),
                Cell::from(Span::styled(state_str, Style::default().fg(state_color))),
                Cell::from(age),
            ])
        })
        .collect();

    let header = Row::new(vec![
        Cell::from(Span::styled(
            " Node",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Cell::from(Span::styled(
            "EDS",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Cell::from(Span::styled(
            "State",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Cell::from(Span::styled(
            "Last seen",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
    ])
    .height(1)
    .bottom_margin(0);

    let table = Table::new(rows, [5, 20, 18, 12])
        .header(header)
        .block(block)
        .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));

    f.render_widget(table, area);
}

fn nmt_state_style(state: &NmtState) -> (&'static str, Color) {
    match state {
        NmtState::Operational => ("OPERATIONAL", Color::Green),
        NmtState::PreOperational => ("PRE-OPERATIONAL", Color::Yellow),
        NmtState::Stopped => ("STOPPED", Color::Red),
        NmtState::Bootup => ("BOOTUP", Color::Blue),
        NmtState::Unknown(_) => ("UNKNOWN", Color::DarkGray),
    }
}

// ─── PDO live values panel ────────────────────────────────────────────────────

/// Render the PDO live values table.
pub fn render_pdo_panel(f: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default()
        .title(" PDO Live Values ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let header = Row::new(vec![
        Cell::from(Span::styled(
            " Node",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Cell::from(Span::styled(
            "PDO",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Cell::from(Span::styled(
            "Signal",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Cell::from(Span::styled(
            "Value",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Cell::from(Span::styled(
            "Updated",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
    ])
    .height(1);

    let mut rows: Vec<Row> = Vec::new();
    let mut keys: Vec<_> = state.pdo_values.keys().copied().collect();
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
                let node_cell = if i == 0 {
                    Cell::from(format!(" {node_id:3}"))
                } else {
                    Cell::from("")
                };
                let pdo_cell = if i == 0 {
                    Cell::from(format!("{pdo_num}"))
                } else {
                    Cell::from("")
                };
                let age_cell = if i == 0 {
                    Cell::from(age_str.clone())
                } else {
                    Cell::from("")
                };

                rows.push(Row::new(vec![
                    node_cell,
                    pdo_cell,
                    Cell::from(v.signal_name.clone()),
                    Cell::from(v.value.to_string()),
                    age_cell,
                ]));
            }
        }
    }

    let table = Table::new(rows, [5, 5, 24, 16, 12])
        .header(header)
        .block(block);

    f.render_widget(table, area);
}

// ─── SDO log panel ────────────────────────────────────────────────────────────

/// Render the SDO event ring buffer as a scrollable log.
pub fn render_sdo_log(f: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default()
        .title(format!(" SDO Log (last {}) ", state.sdo_log.len()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let visible_rows = area.height.saturating_sub(2) as usize;

    // Show the most-recent entries (bottom of the deque).
    let lines: Vec<Line> = state
        .sdo_log
        .iter()
        .rev()
        .take(visible_rows)
        .rev()
        .map(|entry| {
            let ts = entry.ts.format("%H:%M:%S%.3f").to_string();
            let dir = match entry.direction {
                crate::canopen::sdo::SdoDirection::Read => {
                    Span::styled("READ ", Style::default().fg(Color::Cyan))
                }
                crate::canopen::sdo::SdoDirection::Write => {
                    Span::styled("WRITE", Style::default().fg(Color::Magenta))
                }
            };
            let mut spans = vec![
                Span::raw(format!("[{ts}] ")),
                Span::styled(
                    format!("N{:02} ", entry.node_id),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                dir,
                Span::raw(format!(" {:04X}h/{:02X} ", entry.index, entry.subindex)),
                Span::styled(&entry.name, Style::default().fg(Color::White)),
            ];
            if let Some(v) = &entry.value {
                spans.push(Span::raw(format!(" = {v}")));
            }
            if let Some(abort) = entry.abort_code {
                spans.push(Span::styled(
                    format!(" [ABORT 0x{abort:08X}]"),
                    Style::default().fg(Color::Red),
                ));
            }
            Line::from(spans)
        })
        .collect();

    let para = Paragraph::new(lines).block(block);
    f.render_widget(para, area);
}

// ─── Footer / stats bar ───────────────────────────────────────────────────────

/// Render the bottom stats bar.
pub fn render_stats_bar(f: &mut Frame, state: &AppState, area: Rect) {
    let text = format!(
        " Frames/s: {:>6.1}  Total: {:>10}  Log: {}  Press 'q' to quit ",
        state.fps, state.total_frames, state.log_path
    );
    let para = Paragraph::new(text).style(Style::default().bg(Color::DarkGray).fg(Color::White));
    f.render_widget(para, area);
}
