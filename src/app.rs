/// Application state and event types shared between the CAN receive thread
/// and any UI layer (TUI, egui, etc.).
///
/// Deliberately free of any UI framework imports.
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crate::canopen::nmt::NmtState;
use crate::canopen::pdo::PdoValue;
use crate::canopen::sdo::{SdoDirection, SdoValue};

// ─── Type aliases ─────────────────────────────────────────────────────────────

/// `(last seen monotonic, inter-event period)` stored per node in the NMT map.
type NmtTimestamp = (Instant, Option<Duration>);
/// `(live signal values, last-update monotonic, inter-event period)` per PDO.
type PdoEntry = (Vec<PdoValue>, Instant, Option<Duration>);

// ─── Event types ─────────────────────────────────────────────────────────────

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

/// Decoded CAN event passed from the recv thread to the UI thread.
pub enum CanEvent {
    Nmt {
        node_id: u8,
        state: NmtState,
    },
    Sdo(SdoLogEntry),
    Pdo {
        node_id: u8,
        /// COB-ID of the PDO frame that carried these signals.
        cob_id: u16,
        values: Vec<PdoValue>,
    },
    /// Emitted by the recv thread when a master-initiated SDO transfer is sent.
    /// Allows the UI to show a "pending" indicator while waiting for the response.
    SdoPending {
        node_id: u8,
        index: u16,
        subindex: u8,
        direction: SdoDirection,
    },
    /// Sent by the recv thread when the CAN adapter fails to open.
    AdapterError(String),
}

// ─── Application state ───────────────────────────────────────────────────────

/// Application state updated by incoming decoded events.
pub struct AppState {
    /// node_id → (eds basename, nmt state, last heartbeat (monotonic, inter-event period))
    pub node_map: HashMap<u8, (String, NmtState, Option<NmtTimestamp>)>,
    /// (node_id, cob_id) → live values + timing
    pub pdo_values: HashMap<(u8, u16), PdoEntry>,
    /// Ring buffer of recent SDO events.
    pub sdo_log: VecDeque<SdoLogEntry>,
    /// In-flight SDO transfers initiated by the master: node_id → (index, subindex, direction).
    pub pending_sdos: HashMap<u8, (u16, u8, SdoDirection)>,
    /// Total CAN frames received.
    pub total_frames: u64,
    /// Rolling frames-per-second counter.
    pub fps: f64,
    /// Estimated bus load as a percentage (0–100), derived from fps and baud rate.
    pub bus_load: f64,
    /// Baud rate in bps — kept so `record_frame` can re-derive bus load.
    pub baud_rate: u32,
    /// Path of the JSONL log file for display.
    pub log_path: String,
    // Internal FPS tracking.
    fps_window_start: Instant,
    fps_window_count: u64,
}

const SDO_LOG_CAP: usize = 50;
const FPS_WINDOW_SECS: f64 = 2.0;

impl AppState {
    pub fn new(log_path: String, baud_rate: u32) -> Self {
        AppState {
            node_map: HashMap::new(),
            pdo_values: HashMap::new(),
            sdo_log: VecDeque::with_capacity(SDO_LOG_CAP + 1),
            pending_sdos: HashMap::new(),
            total_frames: 0,
            fps: 0.0,
            bus_load: 0.0,
            baud_rate,
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
            // ~125 bits per standard CAN frame (11-bit ID, 8B data, overhead + avg bit stuffing).
            if self.baud_rate > 0 {
                self.bus_load = (self.fps * 125.0 / self.baud_rate as f64 * 100.0).min(100.0);
            }
            self.fps_window_count = 0;
            self.fps_window_start = Instant::now();
        }
    }

    pub fn update_nmt(&mut self, node_id: u8, state: NmtState) {
        let entry = self
            .node_map
            .entry(node_id)
            .or_insert_with(|| (format!("node{node_id}"), NmtState::Unknown(0xFF), None));
        entry.1 = state;
        let period = entry.2.as_ref().map(|(prev, _)| prev.elapsed());
        entry.2 = Some((Instant::now(), period));
    }

    pub fn push_sdo(&mut self, entry: SdoLogEntry) {
        if self.sdo_log.len() >= SDO_LOG_CAP {
            self.sdo_log.pop_front();
        }
        self.sdo_log.push_back(entry);
    }

    pub fn update_pdo(&mut self, node_id: u8, cob_id: u16, values: Vec<PdoValue>) {
        let period = self
            .pdo_values
            .get(&(node_id, cob_id))
            .map(|(_, prev, _)| prev.elapsed());
        self.pdo_values
            .insert((node_id, cob_id), (values, Instant::now(), period));
    }
}

// ─── Event application ───────────────────────────────────────────────────────

/// Apply a single decoded CAN event to the application state.
/// Free of any UI dependency — can be called from any front-end.
pub fn apply_event(state: &mut AppState, ev: CanEvent) {
    state.record_frame();
    match ev {
        CanEvent::Nmt {
            node_id,
            state: nmt_state,
        } => {
            state.update_nmt(node_id, nmt_state);
        }
        CanEvent::Sdo(entry) => {
            // Clear any pending indicator for this node.
            state.pending_sdos.remove(&entry.node_id);
            state.push_sdo(entry);
        }
        CanEvent::Pdo {
            node_id,
            cob_id,
            values,
        } => {
            state.update_pdo(node_id, cob_id, values);
        }
        CanEvent::SdoPending {
            node_id,
            index,
            subindex,
            direction,
        } => {
            state
                .pending_sdos
                .insert(node_id, (index, subindex, direction));
        }
        // Handled directly by the UI layer; nothing to record in AppState.
        CanEvent::AdapterError(_) => {}
    }
}

/// Drain all pending events from `rx` into `state`.
/// Returns `false` if the sender has disconnected (recv thread died).
pub fn drain_events(state: &mut AppState, rx: &mpsc::Receiver<CanEvent>) -> bool {
    loop {
        match rx.try_recv() {
            Ok(ev) => apply_event(state, ev),
            Err(mpsc::TryRecvError::Empty) => return true,
            Err(mpsc::TryRecvError::Disconnected) => return false,
        }
    }
}
