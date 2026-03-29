/// CAN session lifecycle: load EDS, open adapter, spawn recv thread.
///
/// Extracted from `main.rs` so the GUI can start/stop sessions without a CLI.
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use chrono::Utc;

use crate::app::{CanEvent, SdoLogEntry};
use crate::canopen::{
    self, classify_frame, extract_cob_id,
    nmt::{decode_heartbeat, decode_nmt_command},
    pdo::PdoDecoder,
    sdo::decode_sdo,
    FrameType,
};
use crate::eds::{parse_eds, types::ObjectDictionary};
use crate::logger::EventLogger;

// ─── Public API ───────────────────────────────────────────────────────────────

/// Configuration collected from the Connect form.
pub struct SessionConfig {
    pub port: String,
    pub baud: u32,
    /// (node_id, path to .eds file)
    pub nodes: Vec<(u8, PathBuf)>,
    pub log_path: String,
}

/// Load EDS files, open the log, spawn the recv thread.
///
/// `(receiver, node_labels)` returned by [`start`].
pub type SessionResult = Result<(mpsc::Receiver<CanEvent>, Vec<(u8, String)>), String>;

/// Returns `(rx, node_labels)` on success, or a human-readable error string.
/// Adapter open errors are delivered asynchronously via `CanEvent::AdapterError`.
pub fn start(config: SessionConfig) -> SessionResult {
    // ── Load EDS (on the calling thread — fast, errors reported immediately) ─
    let mut node_ods: Vec<(u8, ObjectDictionary)> = Vec::new();
    let mut node_labels: Vec<(u8, String)> = Vec::new();

    for (node_id, eds_path) in &config.nodes {
        let label = eds_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("node{node_id}.eds"));

        let od = parse_eds(eds_path)
            .map_err(|e| format!("Failed to load EDS {}: {e}", eds_path.display()))?;

        node_labels.push((*node_id, label));
        node_ods.push((*node_id, od));
    }

    // ── Build PDO decoders ────────────────────────────────────────────────────
    let pdo_decoders: Vec<(u8, PdoDecoder)> = node_ods
        .iter()
        .map(|(id, od)| (*id, PdoDecoder::from_od(*id, od)))
        .collect();

    // ── Open logger ───────────────────────────────────────────────────────────
    let logger = EventLogger::new(&config.log_path)
        .map_err(|e| format!("Failed to open log file {}: {e}", config.log_path))?;

    // ── Spawn recv thread ─────────────────────────────────────────────────────
    // The adapter is opened inside the thread: host-can does not guarantee the
    // `Adapter` trait object is `Send`, so we open on the OS thread that will use it.
    let (tx, rx) = mpsc::channel::<CanEvent>();
    let port = config.port.clone();
    let baud = config.baud;

    thread::spawn(move || {
        let adapter = match host_can::adapter::get_adapter(&port, baud) {
            Ok(a) => a,
            Err(e) => {
                // Send the error to the GUI, then exit the thread cleanly.
                let _ = tx.send(CanEvent::AdapterError(format!(
                    "Failed to open PCAN-USB channel {port}: {e}\n\
                     Make sure the PCUSB library is installed and the adapter is connected."
                )));
                return;
            }
        };
        recv_loop(adapter, &node_ods, &pdo_decoders, tx, logger);
    });

    Ok((rx, node_labels))
}

// ─── Receive loop ─────────────────────────────────────────────────────────────

fn recv_loop(
    adapter: Box<dyn host_can::adapter::Adapter>,
    ods: &[(u8, ObjectDictionary)],
    pdo_decoders: &[(u8, PdoDecoder)],
    tx: mpsc::Sender<CanEvent>,
    mut logger: EventLogger,
) {
    let timeout = Some(Duration::from_millis(500));

    loop {
        let frame = match adapter.recv(timeout) {
            Ok(f) => f,
            Err(e) => {
                let msg = format!("{e:?}");
                if !msg.contains("ReadTimeout") {
                    eprintln!("CAN recv error: {e:?}");
                }
                continue;
            }
        };

        use embedded_can::Frame;
        if !frame.is_data_frame() {
            continue;
        }

        let data = frame.data();
        let cob_id = extract_cob_id(&frame);
        let ts = Utc::now();

        match classify_frame(cob_id) {
            // ── NMT command (COB-ID 0x000) ────────────────────────────────
            FrameType::NmtCommand => {
                if let Some(ev) = decode_nmt_command(data) {
                    logger.log_nmt(ts, &ev);
                    if let canopen::nmt::NmtEvent::Command {
                        command: _,
                        target_node,
                    } = &ev
                    {
                        let _ = target_node;
                    }
                }
            }

            // ── NMT heartbeat / bootup ────────────────────────────────────
            FrameType::Heartbeat(node_id) => {
                if let Some(ev) = decode_heartbeat(node_id, data) {
                    logger.log_nmt(ts, &ev);
                    if let canopen::nmt::NmtEvent::Heartbeat { node_id, ref state } = ev {
                        if tx
                            .send(CanEvent::Nmt {
                                node_id,
                                state: state.clone(),
                            })
                            .is_err()
                        {
                            return; // GUI disconnected — stop the thread
                        }
                    }
                }
            }

            // ── SDO response (device → master) ────────────────────────────
            FrameType::SdoResponse(node_id) => {
                let od = find_od(ods, node_id);
                if let Some(sdo_ev) = decode_sdo(node_id, data, od, true) {
                    logger.log_sdo(ts, &sdo_ev, data);
                    if tx
                        .send(CanEvent::Sdo(SdoLogEntry {
                            ts,
                            node_id: sdo_ev.node_id,
                            direction: sdo_ev.direction,
                            index: sdo_ev.index,
                            subindex: sdo_ev.subindex,
                            name: sdo_ev.name,
                            value: sdo_ev.value,
                            abort_code: sdo_ev.abort_code,
                        }))
                        .is_err()
                    {
                        return;
                    }
                }
            }

            // ── SDO request (master → device) ─────────────────────────────
            FrameType::SdoRequest(node_id) => {
                let od = find_od(ods, node_id);
                if let Some(sdo_ev) = decode_sdo(node_id, data, od, false) {
                    logger.log_sdo(ts, &sdo_ev, data);
                    if tx
                        .send(CanEvent::Sdo(SdoLogEntry {
                            ts,
                            node_id: sdo_ev.node_id,
                            direction: sdo_ev.direction,
                            index: sdo_ev.index,
                            subindex: sdo_ev.subindex,
                            name: sdo_ev.name,
                            value: sdo_ev.value,
                            abort_code: sdo_ev.abort_code,
                        }))
                        .is_err()
                    {
                        return;
                    }
                }
            }

            // ── TPDO ─────────────────────────────────────────────────────
            FrameType::Tpdo(pdo_num, node_id) => {
                let decoder = find_pdo_decoder(pdo_decoders, node_id);
                if let Some(values) = decoder.and_then(|d| d.decode(cob_id, data)) {
                    logger.log_pdo(ts, node_id, pdo_num, &values, data);
                    if tx
                        .send(CanEvent::Pdo {
                            node_id,
                            pdo_num,
                            values,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }

            // ── RPDO (master → device; log only) ─────────────────────────
            FrameType::Rpdo(pdo_num, node_id) => {
                let decoder = find_pdo_decoder(pdo_decoders, node_id);
                if let Some(values) = decoder.and_then(|d| d.decode(cob_id, data)) {
                    logger.log_pdo(ts, node_id, pdo_num, &values, data);
                }
            }

            _ => {}
        }
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn find_od(ods: &[(u8, ObjectDictionary)], node_id: u8) -> &ObjectDictionary {
    ods.iter()
        .find(|(id, _)| *id == node_id)
        .map(|(_, od)| od)
        .unwrap_or_else(|| {
            ods.first()
                .map(|(_, od)| od)
                .expect("at least one OD required")
        })
}

fn find_pdo_decoder(decoders: &[(u8, PdoDecoder)], node_id: u8) -> Option<&PdoDecoder> {
    decoders
        .iter()
        .find(|(id, _)| *id == node_id)
        .map(|(_, d)| d)
}
