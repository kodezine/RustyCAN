mod widgets;

use std::collections::{HashMap, VecDeque};
use std::io;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend, layout::{Constraint, Direction, Layout}};

use crate::canopen::nmt::NmtState;
use crate::canopen::pdo::PdoValue;
use crate::canopen::sdo::{SdoDirection, SdoValue};

/// An SDO entry kept in the ring buffer for display.
#[derive(Debug, Clone)]
pub struct SdoLogEntry {
    pub ts: DateTime<Utc>,
    pub node_id: u8,
    pub direction: SdoDirection,
    pub index: u16,
    pub subindex: u8,
    pub name: String,
    pub value: Option<SdoValue>,
    pub abort_code: Option<u32>,
}

/// Application state updated by incoming decoded events.
pub struct AppState {
    /// node_id → (eds basename, nmt state, last heartbeat time)
    pub node_map: HashMap<u8, (String, NmtState, Option<Instant>)>,
    /// (node_id, pdo_num) → (live values, last-update time)
    pub pdo_values: HashMap<(u8, u8), (Vec<PdoValue>, Instant)>,
    /// Ring buffer of recent SDO events.
    pub sdo_log: VecDeque<SdoLogEntry>,
    /// Total CAN frames received.
    pub total_frames: u64,
    /// Rolling frames-per-second counter.
    pub fps: f64,
    /// Path of the JSONL log file for display.
    pub log_path: String,
    // Internal FPS tracking.
    fps_window_start: Instant,
    fps_window_count: u64,
}

const SDO_LOG_CAP: usize = 50;
const FPS_WINDOW_SECS: f64 = 2.0;

impl AppState {
    pub fn new(log_path: String) -> Self {
        AppState {
            node_map: HashMap::new(),
            pdo_values: HashMap::new(),
            sdo_log: VecDeque::with_capacity(SDO_LOG_CAP + 1),
            total_frames: 0,
            fps: 0.0,
            log_path,
            fps_window_start: Instant::now(),
            fps_window_count: 0,
        }
    }

    /// Initialise state for configured nodes so the NMT panel shows them
    /// immediately, even before any heartbeat is received.
    pub fn init_nodes(&mut self, nodes: &[(u8, String)]) {
        for (id, eds_name) in nodes {
            self.node_map
                .entry(*id)
                .or_insert_with(|| (eds_name.clone(), NmtState::Unknown(0xFF), None));
        }
    }

    pub fn record_frame(&mut self) {
        self.total_frames += 1;
        self.fps_window_count += 1;
        let elapsed = self.fps_window_start.elapsed().as_secs_f64();
        if elapsed >= FPS_WINDOW_SECS {
            self.fps = self.fps_window_count as f64 / elapsed;
            self.fps_window_count = 0;
            self.fps_window_start = Instant::now();
        }
    }

    pub fn update_nmt(&mut self, node_id: u8, state: NmtState) {
        let entry = self.node_map.entry(node_id).or_insert_with(|| {
            (format!("node{node_id}"), NmtState::Unknown(0xFF), None)
        });
        entry.1 = state;
        entry.2 = Some(Instant::now());
    }

    pub fn push_sdo(&mut self, entry: SdoLogEntry) {
        if self.sdo_log.len() >= SDO_LOG_CAP {
            self.sdo_log.pop_front();
        }
        self.sdo_log.push_back(entry);
    }

    pub fn update_pdo(&mut self, node_id: u8, pdo_num: u8, values: Vec<PdoValue>) {
        self.pdo_values
            .insert((node_id, pdo_num), (values, Instant::now()));
    }
}

/// Decoded CAN event passed from the recv thread to the UI thread.
pub enum CanEvent {
    Nmt {
        node_id: u8,
        state: NmtState,
    },
    Sdo(SdoLogEntry),
    Pdo {
        node_id: u8,
        pdo_num: u8,
        values: Vec<PdoValue>,
    },
}

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
        loop {
            match rx.try_recv() {
                Ok(ev) => apply_event(&mut state, ev),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    // Adapter thread died — redraw once and exit.
                    draw(terminal, &state)?;
                    return Ok(());
                }
            }
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

fn apply_event(state: &mut AppState, ev: CanEvent) {
    state.record_frame();
    match ev {
        CanEvent::Nmt { node_id, state: nmt_state } => {
            state.update_nmt(node_id, nmt_state);
        }
        CanEvent::Sdo(entry) => state.push_sdo(entry),
        CanEvent::Pdo { node_id, pdo_num, values } => {
            state.update_pdo(node_id, pdo_num, values);
        }
    }
}

fn draw(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &AppState,
) -> io::Result<()> {
    terminal.draw(|f| {
        let size = f.area();

        // Vertical split: NMT  |  PDO
        // Then SDO log + stats bar below.
        let top_height = (state.node_map.len() as u16 + 4).min(size.height / 3).max(6);
        let stats_height = 1u16;
        let sdo_height = size
            .height
            .saturating_sub(top_height + stats_height)
            .max(4)
            / 2;
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
