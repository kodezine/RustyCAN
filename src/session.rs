/// CAN session lifecycle: load EDS, open adapter, spawn recv thread.
///
/// Extracted from `main.rs` so the GUI can start/stop sessions without a CLI.
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use chrono::Utc;
use embedded_can::Frame as EmbeddedFrame;
use host_can::frame::CanFrame;

use crate::app::{CanEvent, SdoLogEntry};
use crate::canopen::{
    self, classify_frame, extract_cob_id,
    nmt::{decode_heartbeat, decode_nmt_command, encode_nmt_command, NmtCommand},
    pdo::PdoDecoder,
    sdo::decode_sdo,
    FrameType,
};
use crate::eds::{parse_eds, types::ObjectDictionary};
use crate::logger::EventLogger;

// ─── Public command type ───────────────────────────────────────────────────────

/// Commands that the GUI can send to the running CAN session.
pub enum CanCommand {
    /// Transmit an NMT master command frame (COB-ID 0x000).
    SendNmt {
        command: NmtCommand,
        /// Target node ID; 0x00 broadcasts to all nodes.
        target_node: u8,
    },
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Configuration collected from the Connect form.
pub struct SessionConfig {
    pub port: String,
    pub baud: u32,
    /// `(node_id, optional path to .eds file)`
    pub nodes: Vec<(u8, Option<PathBuf>)>,
    pub log_path: String,
    /// When `true`, the session only receives — no CAN frames are ever transmitted.
    /// All [`CanCommand`] variants (NMT and future SDO/PDO writes) are silently
    /// dropped. Software-level only; the adapter still participates in ACK bits.
    pub listen_only: bool,
}

/// Load EDS files, open the log, spawn the recv thread.
///
/// `(receiver, command_sender, node_labels)` returned by [`start`].
pub type SessionResult = Result<
    (
        mpsc::Receiver<CanEvent>,
        mpsc::Sender<CanCommand>,
        Vec<(u8, String)>,
    ),
    String,
>;

/// Probe whether the CAN adapter is reachable.
///
/// Opens the adapter, immediately drops it, and returns `true` on success.
/// Intended for the Connect-screen dongle-detection poll.
pub fn probe_adapter(port: &str, baud: u32) -> bool {
    host_can::adapter::get_adapter(port, baud).is_ok()
}

/// Returns `(rx, cmd_tx, node_labels)` on success, or a human-readable error string.
/// Adapter open errors are delivered asynchronously via `CanEvent::AdapterError`.
pub fn start(config: SessionConfig) -> SessionResult {
    // ── Load EDS (on the calling thread — fast, errors reported immediately) ─
    let mut node_ods: Vec<(u8, Option<ObjectDictionary>)> = Vec::new();
    let mut node_labels: Vec<(u8, String)> = Vec::new();

    for (node_id, eds_path_opt) in &config.nodes {
        match eds_path_opt {
            Some(eds_path) => {
                let label = eds_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("node{node_id}.eds"));

                let od = parse_eds(eds_path)
                    .map_err(|e| format!("Failed to load EDS {}: {e}", eds_path.display()))?;

                node_labels.push((*node_id, label));
                node_ods.push((*node_id, Some(od)));
            }
            None => {
                node_labels.push((*node_id, "(no EDS)".into()));
                node_ods.push((*node_id, None));
            }
        }
    }

    // ── Build PDO decoders (only for nodes with an EDS) ──────────────────────
    let pdo_decoders: Vec<(u8, PdoDecoder)> = node_ods
        .iter()
        .filter_map(|(id, od_opt)| {
            od_opt
                .as_ref()
                .map(|od| (*id, PdoDecoder::from_od(*id, od)))
        })
        .collect();

    // ── Open logger ───────────────────────────────────────────────────────────
    let logger = EventLogger::new(&config.log_path)
        .map_err(|e| format!("Failed to open log file {}: {e}", config.log_path))?;

    // ── Channels ──────────────────────────────────────────────────────────────
    let (tx, rx) = mpsc::channel::<CanEvent>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<CanCommand>();

    // ── Spawn recv thread ─────────────────────────────────────────────────────
    // The adapter is opened inside the thread: host-can does not guarantee the
    // `Adapter` trait object is `Send`, so we open on the OS thread that will use it.
    let port = config.port.clone();
    let baud = config.baud;
    let listen_only = config.listen_only;

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
        recv_loop(
            adapter,
            &node_ods,
            &pdo_decoders,
            tx,
            cmd_rx,
            logger,
            listen_only,
        );
    });

    Ok((rx, cmd_tx, node_labels))
}

// ─── Receive loop ─────────────────────────────────────────────────────────────

fn recv_loop(
    adapter: Box<dyn host_can::adapter::Adapter>,
    ods: &[(u8, Option<ObjectDictionary>)],
    pdo_decoders: &[(u8, PdoDecoder)],
    tx: mpsc::Sender<CanEvent>,
    cmd_rx: mpsc::Receiver<CanCommand>,
    mut logger: EventLogger,
    listen_only: bool,
) {
    let timeout = Some(Duration::from_millis(500));

    loop {
        // ── Drain outbound commands from GUI ─────────────────────────────────
        // In listen-only mode, drain and discard every command — prevents the
        // channel from backing up. Any future SendSdo / SendPdo variants are
        // automatically blocked here without further changes.
        if listen_only {
            while cmd_rx.try_recv().is_ok() {}
        } else {
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    CanCommand::SendNmt {
                        ref command,
                        target_node,
                    } => {
                        let payload = encode_nmt_command(command, target_node);
                        if let Some(frame) = host_can::id::new_standard(0x000)
                            .and_then(|id| CanFrame::new(id, &payload))
                        {
                            if let Err(e) = adapter.send(&frame) {
                                eprintln!("NMT send error: {e:?}");
                            } else {
                                logger.log_nmt_sent(Utc::now(), command, target_node, &payload);
                            }
                        }
                    }
                }
            }
        }

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
                    logger.log_nmt(ts, &ev, data, cob_id);
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
                    logger.log_nmt(ts, &ev, data, cob_id);
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
                let empty_od = ObjectDictionary::new();
                let od = find_od(ods, node_id).unwrap_or(&empty_od);
                if let Some(sdo_ev) = decode_sdo(node_id, data, od, true) {
                    logger.log_sdo(ts, &sdo_ev, data, cob_id);
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
                let empty_od = ObjectDictionary::new();
                let od = find_od(ods, node_id).unwrap_or(&empty_od);
                if let Some(sdo_ev) = decode_sdo(node_id, data, od, false) {
                    logger.log_sdo(ts, &sdo_ev, data, cob_id);
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
            FrameType::Tpdo(frame_pdo_num, frame_node_id) => {
                let (node_id, eff_pdo_num, values) = if let Some((actual_id, d)) =
                    find_pdo_decoder_for_cob_id(pdo_decoders, cob_id)
                {
                    let pn = d.pdo_num_for_cob_id(cob_id).unwrap_or(frame_pdo_num);
                    let vals = d
                        .decode(cob_id, data)
                        .unwrap_or_else(|| raw_pdo_signals(data));
                    (actual_id, pn, vals)
                } else {
                    let vals = find_pdo_decoder(pdo_decoders, frame_node_id)
                        .and_then(|d| d.decode(cob_id, data))
                        .unwrap_or_else(|| raw_pdo_signals(data));
                    (frame_node_id, frame_pdo_num, vals)
                };
                logger.log_pdo(ts, node_id, eff_pdo_num, &values, data, cob_id);
                if tx
                    .send(CanEvent::Pdo {
                        node_id,
                        cob_id,
                        values,
                    })
                    .is_err()
                {
                    return;
                }
            }

            // ── RPDO (master → device) ────────────────────────────────────
            FrameType::Rpdo(frame_pdo_num, frame_node_id) => {
                let (node_id, eff_pdo_num, values) = if let Some((actual_id, d)) =
                    find_pdo_decoder_for_cob_id(pdo_decoders, cob_id)
                {
                    let pn = d.pdo_num_for_cob_id(cob_id).unwrap_or(frame_pdo_num);
                    let vals = d
                        .decode(cob_id, data)
                        .unwrap_or_else(|| raw_pdo_signals(data));
                    (actual_id, pn, vals)
                } else {
                    let vals = find_pdo_decoder(pdo_decoders, frame_node_id)
                        .and_then(|d| d.decode(cob_id, data))
                        .unwrap_or_else(|| raw_pdo_signals(data));
                    (frame_node_id, frame_pdo_num, vals)
                };
                logger.log_pdo(ts, node_id, eff_pdo_num, &values, data, cob_id);
                if tx
                    .send(CanEvent::Pdo {
                        node_id,
                        cob_id,
                        values,
                    })
                    .is_err()
                {
                    return;
                }
            }

            _ => {}
        }
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Find the `ObjectDictionary` for a node, or `None` if no EDS was loaded for it.
fn find_od(ods: &[(u8, Option<ObjectDictionary>)], node_id: u8) -> Option<&ObjectDictionary> {
    ods.iter()
        .find(|(id, _)| *id == node_id)
        .and_then(|(_, od_opt)| od_opt.as_ref())
}

fn find_pdo_decoder(decoders: &[(u8, PdoDecoder)], node_id: u8) -> Option<&PdoDecoder> {
    decoders
        .iter()
        .find(|(id, _)| *id == node_id)
        .map(|(_, d)| d)
}

/// Find a PDO decoder by searching all loaded decoders' COB-ID mapping tables.
/// This correctly handles custom COB-IDs that don't match the default range.
fn find_pdo_decoder_for_cob_id(
    decoders: &[(u8, PdoDecoder)],
    cob_id: u16,
) -> Option<(u8, &PdoDecoder)> {
    decoders
        .iter()
        .find(|(_, d)| d.mappings.contains_key(&cob_id))
        .map(|(id, d)| (*id, d))
}

/// Build synthesised PDO signals from raw frame bytes when no EDS decoder exists.
/// Each byte is labelled `Byte0`, `Byte1`, … and formatted as a single-byte hex value.
fn raw_pdo_signals(data: &[u8]) -> Vec<crate::canopen::pdo::PdoValue> {
    use crate::canopen::pdo::{PdoRawValue, PdoValue};
    data.iter()
        .enumerate()
        .map(|(i, b)| PdoValue {
            signal_name: format!("Byte{i}"),
            value: PdoRawValue::Bytes(vec![*b]),
        })
        .collect()
}
