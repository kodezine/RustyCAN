mod widgets;

use std::io;
use std::sync::mpsc;
use std::time::Duration;

use crate::app::{drain_events, AppState, CanEvent};
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

/// Run the TUI event loop.
///
/// Exits when the user presses `q` or `Ctrl-C`.  Frames arrive on `rx` from
/// a background thread; they are processed in batches between renders.
pub fn run(
    rx: mpsc::Receiver<CanEvent>,
    state: AppState,
    nodes: Vec<(u8, String)>,
) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, rx, state, nodes);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    rx: mpsc::Receiver<CanEvent>,
    mut state: AppState,
    nodes: Vec<(u8, String)>,
) -> io::Result<()> {
    state.init_nodes(&nodes);

    let tick = Duration::from_millis(100);

    loop {
        // Drain all pending decoded CAN events (non-blocking).
        if !drain_events(&mut state, &rx) {
            // Adapter thread died — redraw once and exit.
            draw(terminal, &state)?;
            return Ok(());
        }

        // Render.
        draw(terminal, &state)?;

        // Poll keyboard for up to `tick` duration.
        if event::poll(tick)? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(()),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }
    }
}


fn draw(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, state: &AppState) -> io::Result<()> {
    terminal.draw(|f| {
        let size = f.area();

        // Vertical split: NMT  |  PDO
        // Then SDO log + stats bar below.
        let top_height = (state.node_map.len() as u16 + 4)
            .min(size.height / 3)
            .max(6);
        let stats_height = 1u16;
        let sdo_height = size.height.saturating_sub(top_height + stats_height).max(4) / 2;
        let pdo_height = size
            .height
            .saturating_sub(top_height + stats_height + sdo_height)
            .max(4);

        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(top_height),
                Constraint::Length(pdo_height),
                Constraint::Length(sdo_height),
                Constraint::Length(stats_height),
            ])
            .split(size);

        widgets::render_nmt_panel(f, state, vertical[0]);
        widgets::render_pdo_panel(f, state, vertical[1]);
        widgets::render_sdo_log(f, state, vertical[2]);
        widgets::render_stats_bar(f, state, vertical[3]);
    })?;
    Ok(())
}
