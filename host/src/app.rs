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
use crate::dbc::types::{DbcByteOrder, DbcFrameSignals, DbcValueType};

// ─── Type aliases ─────────────────────────────────────────────────────────────

/// `(last seen monotonic, inter-event period)` stored per node in the NMT map.
type NmtTimestamp = (Instant, Option<Duration>);
/// `(live signal values, last-update monotonic, inter-event period)` per PDO.
type PdoEntry = (Vec<PdoValue>, Instant, Option<Duration>);

// ─── DBC state ────────────────────────────────────────────────────────────────

/// Live state for one decoded DBC message.
pub struct DbcMessageEntry {
    pub message_name: String,
    /// Most-recently decoded signal values for this message.
    pub values: crate::dbc::types::DbcSignalValue,
}

/// Live decoded values for a single DBC signal (keyed by signal name).
pub struct DbcSignalEntry {
    pub signal_name: String,
    pub can_id: u32,
    pub message_name: String,
    pub raw_int: i64,
    pub physical: f64,
    pub unit: String,
    pub description: Option<String>,
    pub last_seen: Instant,
    pub count: u64,
    // Encoding metadata (populated from DbcSignalDef on first decode).
    pub start_bit: u64,
    pub bit_size: u64,
    pub byte_order: DbcByteOrder,
    pub value_type: DbcValueType,
    pub factor: f64,
    pub offset: f64,
    pub dlc: u8,
}

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
    /// Dongle was physically disconnected; session is polling for reconnect.
    AdapterDisconnected,
    /// Dongle has been reconnected and the session has resumed.
    AdapterReconnected,
    /// Device firmware version reported by GET_INFO immediately after adapter open.
    /// Carried as `(major, minor, patch)` for display and update-check in the TUI.
    FirmwareVersion(u8, u8, u8),
    /// Emitted after a successful auto-baud detection pass.
    /// Carries the detected nominal CAN bitrate in bits/s.
    AutoBaudDetected(u32),
    /// Signals decoded from a CAN frame against the loaded DBC database.
    DbcSignal(DbcFrameSignals),
    /// Emitted once when the DBC database is loaded successfully.
    /// Carries the DBC file's stem name (e.g. `"sample_bus"`).
    DbcLoaded(String),
    /// A CAN frame that was not matched by any DBC or CANopen decoder.
    /// Carries the COB-ID, up to 8 data bytes, and the source CAN channel
    /// (0 = FDCAN1, 1 = FDCAN2; always 0 for PEAK).
    RawFrame {
        cob_id: u32,
        data: Vec<u8>,
        /// Source CAN channel: 0 = FDCAN1, 1 = FDCAN2.
        port: u8,
    },
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
    /// Live DBC signal values: (can_id, signal_name) → entry.
    ///
    /// Populated when a DBC file is loaded and matching CAN frames are received.
    pub dbc_signals: HashMap<(u32, String), DbcSignalEntry>,
    /// Most-recently received raw CAN payload per message CAN ID.
    ///
    /// Used to construct outbound frames: other signals' bits are preserved.
    pub last_raw_bytes: HashMap<u32, Vec<u8>>,
    /// Filename stem of the loaded DBC (e.g. `"sample_bus"`), or `None` if no DBC is loaded.
    pub dbc_loaded: Option<String>,
    /// Total CAN frames received.
    pub total_frames: u64,
    /// Rolling frames-per-second counter.
    pub fps: f64,
    /// Estimated bus load as a percentage (0–100), derived from fps and baud rate.
    pub bus_load: f64,
    /// Baud rate in bps — kept so `record_frame` can re-derive bus load.
    pub baud_rate: u32,
    /// Firmware version reported by the connected device, or `None` before the
    /// first GET_INFO response arrives (or for non-KCAN adapters).
    pub device_fw_version: Option<(u8, u8, u8)>,
    /// Baud rate detected by auto-baud, or `None` if baud was configured manually.
    pub detected_baud: Option<u32>,
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
            dbc_signals: HashMap::new(),
            last_raw_bytes: HashMap::new(),
            dbc_loaded: None,
            total_frames: 0,
            fps: 0.0,
            bus_load: 0.0,
            baud_rate,
            device_fw_version: None,
            detected_baud: None,
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
        CanEvent::AdapterError(_)
        | CanEvent::AdapterDisconnected
        | CanEvent::AdapterReconnected => {}
        CanEvent::FirmwareVersion(maj, min, pat) => {
            state.device_fw_version = Some((maj, min, pat));
        }
        CanEvent::AutoBaudDetected(baud) => {
            state.detected_baud = Some(baud);
            state.baud_rate = baud;
        }
        CanEvent::DbcLoaded(name) => {
            state.dbc_loaded = Some(name);
        }
        // Raw frames not decoded by DBC or CANopen — nothing to record in state.
        CanEvent::RawFrame { .. } => {}
        CanEvent::DbcSignal(frame_signals) => {
            let now = Instant::now();
            // Store the raw payload so the GUI can use it as a base for writes.
            state
                .last_raw_bytes
                .insert(frame_signals.can_id, frame_signals.raw_data.clone());
            for sig in frame_signals.values {
                let key = (frame_signals.can_id, sig.signal_name.clone());
                let entry = state
                    .dbc_signals
                    .entry(key)
                    .or_insert_with(|| DbcSignalEntry {
                        signal_name: sig.signal_name.clone(),
                        can_id: frame_signals.can_id,
                        message_name: frame_signals.message_name.clone(),
                        raw_int: 0,
                        physical: 0.0,
                        unit: sig.unit.clone(),
                        description: None,
                        last_seen: now,
                        count: 0,
                        start_bit: 0,
                        bit_size: 0,
                        byte_order: DbcByteOrder::LittleEndian,
                        value_type: DbcValueType::Unsigned,
                        factor: 1.0,
                        offset: 0.0,
                        dlc: 8,
                    });
                entry.raw_int = sig.raw_int;
                entry.physical = sig.physical;
                entry.unit = sig.unit;
                entry.description = sig.description;
                entry.last_seen = now;
                entry.count += 1;
                // Update encoding metadata whenever fresh data arrives.
                if let Some(def) = sig.encoding_def {
                    entry.start_bit = def.start_bit;
                    entry.bit_size = def.bit_size;
                    entry.byte_order = def.byte_order;
                    entry.value_type = def.value_type;
                    entry.factor = def.factor;
                    entry.offset = def.offset;
                    entry.dlc = def.dlc;
                }
            }
        }
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
