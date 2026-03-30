/// CAN session lifecycle: load EDS, open adapter, spawn recv thread.
///
/// Extracted from `main.rs` so the GUI can start/stop sessions without a CLI.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use embedded_can::Frame as EmbeddedFrame;
use host_can::frame::CanFrame;

use crate::app::{CanEvent, SdoLogEntry};
use crate::canopen::{
    self, classify_frame, extract_cob_id,
    nmt::{decode_heartbeat, decode_nmt_command, encode_nmt_command, NmtCommand},
    pdo::PdoDecoder,
    sdo::{
        decode_sdo, decode_segmented_upload_initiate, decode_upload_segment_response,
        encode_download_expedited, encode_download_initiate_segmented, encode_download_segment,
        encode_upload_request, encode_upload_segment_ack, interpret_value,
        is_download_initiate_ack, is_download_segment_ack, SdoDirection,
    },
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
    /// Initiate an SDO upload (master reads from node). COB-ID 0x600+node_id.
    SdoRead {
        node_id: u8,
        index: u16,
        subindex: u8,
    },
    /// Initiate an SDO download (master writes to node). COB-ID 0x600+node_id.
    SdoWrite {
        node_id: u8,
        index: u16,
        subindex: u8,
        data: Vec<u8>,
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
    /// How long (milliseconds) to wait for an SDO response before emitting a
    /// synthetic abort event with code 0x05040000 (protocol timed out).
    pub sdo_timeout_ms: u64,
}

/// Load EDS files, open the log, spawn the recv thread.
///
/// `(receiver, command_sender, node_labels)` returned by [`start`].
pub type SessionResult = Result<
    (
        mpsc::Receiver<CanEvent>,
        mpsc::Sender<CanCommand>,
        Vec<(u8, String)>,
        String, // Actual log file path with timestamp
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
    // Compute the timestamped log path before creating the logger
    let actual_log_path =
        crate::logger::add_timestamp_to_path(std::path::Path::new(&config.log_path))
            .to_string_lossy()
            .to_string();

    let logger = EventLogger::new(&config.log_path)
        .map_err(|e| format!("Failed to open log file {}: {e}", actual_log_path))?;

    // ── Channels ──────────────────────────────────────────────────────────────
    let (tx, rx) = mpsc::channel::<CanEvent>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<CanCommand>();

    // ── Spawn recv thread ─────────────────────────────────────────────────────
    // The adapter is opened inside the thread: host-can does not guarantee the
    // `Adapter` trait object is `Send`, so we open on the OS thread that will use it.
    let port = config.port.clone();
    let baud = config.baud;
    let listen_only = config.listen_only;
    let sdo_timeout_ms = config.sdo_timeout_ms;

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
            tx.clone(),
            cmd_rx,
            logger,
            listen_only,
            sdo_timeout_ms,
            &port,
            baud,
        );
    });

    Ok((rx, cmd_tx, node_labels, actual_log_path))
}

// ─── Receive loop ─────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn recv_loop(
    mut adapter: Box<dyn host_can::adapter::Adapter>,
    ods: &[(u8, Option<ObjectDictionary>)],
    pdo_decoders: &[(u8, PdoDecoder)],
    tx: mpsc::Sender<CanEvent>,
    cmd_rx: mpsc::Receiver<CanCommand>,
    mut logger: EventLogger,
    listen_only: bool,
    sdo_timeout_ms: u64,
    port: &str,
    baud: u32,
) {
    let timeout = Some(Duration::from_millis(500));

    // Error tracking for automatic recovery
    let mut consecutive_errors = 0u32;
    const MAX_CONSECUTIVE_ERRORS: u32 = 10;
    let mut last_error_time = Instant::now();

    // ── Per-node in-flight SDO tracking ──────────────────────────────────────
    /// Internal state for the active SDO transfer on one node.
    enum SdoPendingState {
        /// Waiting for the server's initiate response (expedited upload or expedited download).
        WaitingResponse,
        /// Segmented upload in progress: accumulating server segments.
        UploadSegmented {
            toggle: bool,
            buf: Vec<u8>,
            expected_size: Option<u32>,
        },
        /// Segmented download in progress: sending chunks to the server.
        DownloadSegmented { remaining: Vec<u8>, toggle: bool },
    }

    struct PendingSdo {
        #[allow(dead_code)]
        node_id: u8,
        index: u16,
        subindex: u8,
        direction: SdoDirection,
        started_at: Instant,
        state: SdoPendingState,
    }

    let mut pending_sdos: HashMap<u8, PendingSdo> = HashMap::new();

    loop {
        // ── Drain outbound commands from GUI ─────────────────────────────────
        // In listen-only mode, drain and discard every command — prevents the
        // channel from backing up.
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

                    CanCommand::SdoRead {
                        node_id,
                        index,
                        subindex,
                    } => {
                        let payload = encode_upload_request(index, subindex);
                        let cob_id = 0x600u16 + node_id as u16;
                        if let Some(frame) = host_can::id::new_standard(cob_id)
                            .and_then(|id| CanFrame::new(id, &payload))
                        {
                            match adapter.send(&frame) {
                                Ok(_) => {
                                    pending_sdos.insert(
                                        node_id,
                                        PendingSdo {
                                            node_id,
                                            index,
                                            subindex,
                                            direction: SdoDirection::Read,
                                            started_at: Instant::now(),
                                            state: SdoPendingState::WaitingResponse,
                                        },
                                    );
                                    let _ = tx.send(CanEvent::SdoPending {
                                        node_id,
                                        index,
                                        subindex,
                                        direction: SdoDirection::Read,
                                    });
                                }
                                Err(e) => {
                                    eprintln!("SDO read send error: {e:?}");
                                    // Send an abort event to notify the GUI
                                    let empty_od = ObjectDictionary::new();
                                    let od = find_od(ods, node_id).unwrap_or(&empty_od);
                                    let name = od
                                        .get(&(index, subindex))
                                        .map(|e| e.name.clone())
                                        .unwrap_or_else(|| format!("{index:04X}h/{subindex:02X}"));
                                    let _ = tx.send(CanEvent::Sdo(SdoLogEntry {
                                        ts: Utc::now(),
                                        node_id,
                                        direction: SdoDirection::Read,
                                        index,
                                        subindex,
                                        name,
                                        value: None,
                                        abort_code: Some(0x08000000), // General error
                                    }));
                                }
                            }
                        }
                    }

                    CanCommand::SdoWrite {
                        node_id,
                        index,
                        subindex,
                        data,
                    } => {
                        let cob_id = 0x600u16 + node_id as u16;
                        if data.len() <= 4 {
                            // Expedited download
                            if let Some(payload) = encode_download_expedited(index, subindex, &data)
                            {
                                if let Some(frame) = host_can::id::new_standard(cob_id)
                                    .and_then(|id| CanFrame::new(id, &payload))
                                {
                                    match adapter.send(&frame) {
                                        Ok(_) => {
                                            pending_sdos.insert(
                                                node_id,
                                                PendingSdo {
                                                    node_id,
                                                    index,
                                                    subindex,
                                                    direction: SdoDirection::Write,
                                                    started_at: Instant::now(),
                                                    state: SdoPendingState::WaitingResponse,
                                                },
                                            );
                                            let _ = tx.send(CanEvent::SdoPending {
                                                node_id,
                                                index,
                                                subindex,
                                                direction: SdoDirection::Write,
                                            });
                                        }
                                        Err(e) => {
                                            eprintln!("SDO write send error: {e:?}");
                                            // Send an abort event to notify the GUI
                                            let empty_od = ObjectDictionary::new();
                                            let od = find_od(ods, node_id).unwrap_or(&empty_od);
                                            let name = od
                                                .get(&(index, subindex))
                                                .map(|e| e.name.clone())
                                                .unwrap_or_else(|| {
                                                    format!("{index:04X}h/{subindex:02X}")
                                                });
                                            let _ = tx.send(CanEvent::Sdo(SdoLogEntry {
                                                ts: Utc::now(),
                                                node_id,
                                                direction: SdoDirection::Write,
                                                index,
                                                subindex,
                                                name,
                                                value: None,
                                                abort_code: Some(0x08000000), // General error
                                            }));
                                        }
                                    }
                                }
                            }
                        } else {
                            // Segmented download initiate
                            let size = data.len() as u32;
                            let payload = encode_download_initiate_segmented(index, subindex, size);
                            if let Some(frame) = host_can::id::new_standard(cob_id)
                                .and_then(|id| CanFrame::new(id, &payload))
                            {
                                match adapter.send(&frame) {
                                    Ok(_) => {
                                        pending_sdos.insert(
                                            node_id,
                                            PendingSdo {
                                                node_id,
                                                index,
                                                subindex,
                                                direction: SdoDirection::Write,
                                                started_at: Instant::now(),
                                                state: SdoPendingState::DownloadSegmented {
                                                    remaining: data,
                                                    toggle: false,
                                                },
                                            },
                                        );
                                        let _ = tx.send(CanEvent::SdoPending {
                                            node_id,
                                            index,
                                            subindex,
                                            direction: SdoDirection::Write,
                                        });
                                    }
                                    Err(e) => {
                                        eprintln!("SDO segmented initiate error: {e:?}");
                                        // Send an abort event to notify the GUI
                                        let empty_od = ObjectDictionary::new();
                                        let od = find_od(ods, node_id).unwrap_or(&empty_od);
                                        let name = od
                                            .get(&(index, subindex))
                                            .map(|e| e.name.clone())
                                            .unwrap_or_else(|| {
                                                format!("{index:04X}h/{subindex:02X}")
                                            });
                                        let _ = tx.send(CanEvent::Sdo(SdoLogEntry {
                                            ts: Utc::now(),
                                            node_id,
                                            direction: SdoDirection::Write,
                                            index,
                                            subindex,
                                            name,
                                            value: None,
                                            abort_code: Some(0x08000000), // General error
                                        }));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let frame = match adapter.recv(timeout) {
            Ok(f) => {
                // Reset error counter on successful receive
                consecutive_errors = 0;
                f
            }
            Err(e) => {
                let msg = format!("{e:?}");

                // ReadTimeout is normal when there's no traffic, don't count it as an error
                if msg.contains("ReadTimeout") {
                    continue;
                }

                // Log the error
                eprintln!("CAN recv error: {e:?}");

                // Track consecutive errors
                if last_error_time.elapsed() < Duration::from_secs(1) {
                    consecutive_errors += 1;
                } else {
                    consecutive_errors = 1;
                }
                last_error_time = Instant::now();

                // If we're getting too many errors in quick succession, try to recover
                if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                    eprintln!("Too many consecutive CAN errors ({consecutive_errors}), attempting recovery...");

                    // Try to reinitialize the adapter
                    match host_can::adapter::get_adapter(port, baud) {
                        Ok(new_adapter) => {
                            eprintln!("Successfully reconnected to CAN adapter");
                            adapter = new_adapter;
                            consecutive_errors = 0;

                            // Clear pending SDOs since we lost connection
                            pending_sdos.clear();
                        }
                        Err(e) => {
                            eprintln!("Failed to reconnect to CAN adapter: {e:?}");
                            let _ = tx.send(CanEvent::AdapterError(format!(
                                "Lost connection to CAN adapter after repeated errors.\n\
                                 Last error: {e:?}\n\
                                 The adapter may need to be physically reset or unplugged/replugged."
                            )));
                            return; // Exit the thread
                        }
                    }
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

                if let Some(mut pending) = pending_sdos.remove(&node_id) {
                    // Route through the SDO state machine.
                    let cs = data.first().copied().unwrap_or(0);

                    // ── Abort (CS=0x80) — always terminates the transfer ──────
                    if cs == 0x80 {
                        let abort_code = u32::from_le_bytes([
                            data.get(4).copied().unwrap_or(0),
                            data.get(5).copied().unwrap_or(0),
                            data.get(6).copied().unwrap_or(0),
                            data.get(7).copied().unwrap_or(0),
                        ]);
                        let name = od
                            .get(&(pending.index, pending.subindex))
                            .map(|e| e.name.clone())
                            .unwrap_or_else(|| {
                                format!("{:04X}h/{:02X}", pending.index, pending.subindex)
                            });
                        let entry = SdoLogEntry {
                            ts,
                            node_id,
                            direction: pending.direction,
                            index: pending.index,
                            subindex: pending.subindex,
                            name,
                            value: None,
                            abort_code: Some(abort_code),
                        };
                        logger.log_sdo(
                            ts,
                            &crate::canopen::sdo::SdoEvent {
                                node_id,
                                direction: entry.direction.clone(),
                                index: entry.index,
                                subindex: entry.subindex,
                                name: entry.name.clone(),
                                value: None,
                                abort_code: Some(abort_code),
                            },
                            data,
                            cob_id,
                        );
                        if tx.send(CanEvent::Sdo(entry)).is_err() {
                            return;
                        }
                        // pending already removed above
                    } else {
                        match pending.state {
                            // ── Waiting for initiate response ─────────────
                            SdoPendingState::WaitingResponse => {
                                // Expedited upload response
                                if matches!(cs, 0x43 | 0x47 | 0x4B | 0x4F) {
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
                                    // pending removed — transfer complete
                                }
                                // Download expedited ack (CS=0x60 with index/subindex echo)
                                else if is_download_initiate_ack(data) {
                                    let name = od
                                        .get(&(pending.index, pending.subindex))
                                        .map(|e| e.name.clone())
                                        .unwrap_or_else(|| {
                                            format!(
                                                "{:04X}h/{:02X}",
                                                pending.index, pending.subindex
                                            )
                                        });
                                    let entry = SdoLogEntry {
                                        ts,
                                        node_id,
                                        direction: SdoDirection::Write,
                                        index: pending.index,
                                        subindex: pending.subindex,
                                        name,
                                        value: None,
                                        abort_code: None,
                                    };
                                    logger.log_sdo(
                                        ts,
                                        &crate::canopen::sdo::SdoEvent {
                                            node_id,
                                            direction: entry.direction.clone(),
                                            index: entry.index,
                                            subindex: entry.subindex,
                                            name: entry.name.clone(),
                                            value: None,
                                            abort_code: None,
                                        },
                                        data,
                                        cob_id,
                                    );
                                    if tx.send(CanEvent::Sdo(entry)).is_err() {
                                        return;
                                    }
                                    // pending removed — transfer complete
                                }
                                // Segmented upload initiate (CS=0x40 or 0x41)
                                else if let Some(opt_size) =
                                    decode_segmented_upload_initiate(data)
                                {
                                    // Send first upload segment request (toggle=false)
                                    let cob_out = 0x600u16 + node_id as u16;
                                    let ack_frame = encode_upload_segment_ack(false);
                                    if let Some(f) = host_can::id::new_standard(cob_out)
                                        .and_then(|id| CanFrame::new(id, &ack_frame))
                                    {
                                        let _ = adapter.send(&f);
                                    }
                                    // Transition to UploadSegmented
                                    pending.state = SdoPendingState::UploadSegmented {
                                        toggle: false,
                                        buf: Vec::new(),
                                        expected_size: opt_size,
                                    };
                                    pending_sdos.insert(node_id, pending);
                                } else {
                                    // Unexpected frame for this state; reinsert so timeout still fires.
                                    pending_sdos.insert(node_id, pending);
                                }
                            }

                            // ── Accumulating upload segments ──────────────
                            SdoPendingState::UploadSegmented {
                                mut toggle,
                                mut buf,
                                expected_size,
                            } => {
                                if let Some((chunk, is_last)) = decode_upload_segment_response(data)
                                {
                                    buf.extend_from_slice(&chunk);

                                    if is_last {
                                        // Full data assembled — decode typed value
                                        let opt_dtype = od
                                            .get(&(pending.index, pending.subindex))
                                            .map(|e| &e.data_type);
                                        let value = interpret_value(&buf, opt_dtype);
                                        let name = od
                                            .get(&(pending.index, pending.subindex))
                                            .map(|e| e.name.clone())
                                            .unwrap_or_else(|| {
                                                format!(
                                                    "{:04X}h/{:02X}",
                                                    pending.index, pending.subindex
                                                )
                                            });
                                        let entry = SdoLogEntry {
                                            ts,
                                            node_id,
                                            direction: SdoDirection::Read,
                                            index: pending.index,
                                            subindex: pending.subindex,
                                            name,
                                            value: Some(value),
                                            abort_code: None,
                                        };
                                        logger.log_sdo(
                                            ts,
                                            &crate::canopen::sdo::SdoEvent {
                                                node_id,
                                                direction: entry.direction.clone(),
                                                index: entry.index,
                                                subindex: entry.subindex,
                                                name: entry.name.clone(),
                                                value: entry.value.clone(),
                                                abort_code: None,
                                            },
                                            data,
                                            cob_id,
                                        );
                                        if tx.send(CanEvent::Sdo(entry)).is_err() {
                                            return;
                                        }
                                        // pending removed — transfer complete
                                    } else {
                                        // Send next segment request with toggled bit
                                        toggle = !toggle;
                                        let cob_out = 0x600u16 + node_id as u16;
                                        let ack_frame = encode_upload_segment_ack(toggle);
                                        if let Some(f) = host_can::id::new_standard(cob_out)
                                            .and_then(|id| CanFrame::new(id, &ack_frame))
                                        {
                                            let _ = adapter.send(&f);
                                        }
                                        // Reinsert with updated state
                                        pending.state = SdoPendingState::UploadSegmented {
                                            toggle,
                                            buf,
                                            expected_size,
                                        };
                                        pending_sdos.insert(node_id, pending);
                                    }
                                } else {
                                    // Unexpected; reinsert for timeout
                                    pending.state = SdoPendingState::UploadSegmented {
                                        toggle,
                                        buf,
                                        expected_size,
                                    };
                                    pending_sdos.insert(node_id, pending);
                                }
                            }

                            // ── Sending download segments ─────────────────
                            // `toggle` = next toggle bit to use for outgoing segment.
                            // `is_download_initiate_ack` → send first segment.
                            // `is_download_segment_ack(!toggle)` → server acked last segment.
                            SdoPendingState::DownloadSegmented {
                                mut remaining,
                                mut toggle,
                            } => {
                                let send_next_segment = is_download_initiate_ack(data)
                                    || is_download_segment_ack(data, !toggle);

                                if send_next_segment {
                                    let chunk_len = remaining.len().min(7);
                                    let chunk = remaining[..chunk_len].to_vec();
                                    remaining = remaining[chunk_len..].to_vec();
                                    let is_last = remaining.is_empty();

                                    let seg_frame =
                                        encode_download_segment(&chunk, toggle, is_last);
                                    let cob_out = 0x600u16 + node_id as u16;
                                    if let Some(f) = host_can::id::new_standard(cob_out)
                                        .and_then(|id| CanFrame::new(id, &seg_frame))
                                    {
                                        let _ = adapter.send(&f);
                                    }

                                    if is_last {
                                        // Server will ack this last segment; stay pending to
                                        // receive that final ack, but now as WaitingResponse.
                                        // We repurpose WaitingResponse here: on CS=0x20|(toggle<<4)
                                        // we emit success.
                                        pending.state = SdoPendingState::DownloadSegmented {
                                            remaining: Vec::new(),
                                            toggle, // toggle of the last sent segment
                                        };
                                        pending_sdos.insert(node_id, pending);
                                    } else {
                                        toggle = !toggle;
                                        pending.state = SdoPendingState::DownloadSegmented {
                                            remaining,
                                            toggle,
                                        };
                                        pending_sdos.insert(node_id, pending);
                                    }
                                } else if remaining.is_empty()
                                    && is_download_segment_ack(data, toggle)
                                {
                                    // Final segment ack — transfer complete
                                    let name = od
                                        .get(&(pending.index, pending.subindex))
                                        .map(|e| e.name.clone())
                                        .unwrap_or_else(|| {
                                            format!(
                                                "{:04X}h/{:02X}",
                                                pending.index, pending.subindex
                                            )
                                        });
                                    let entry = SdoLogEntry {
                                        ts,
                                        node_id,
                                        direction: SdoDirection::Write,
                                        index: pending.index,
                                        subindex: pending.subindex,
                                        name,
                                        value: None,
                                        abort_code: None,
                                    };
                                    logger.log_sdo(
                                        ts,
                                        &crate::canopen::sdo::SdoEvent {
                                            node_id,
                                            direction: entry.direction.clone(),
                                            index: entry.index,
                                            subindex: entry.subindex,
                                            name: entry.name.clone(),
                                            value: None,
                                            abort_code: None,
                                        },
                                        data,
                                        cob_id,
                                    );
                                    if tx.send(CanEvent::Sdo(entry)).is_err() {
                                        return;
                                    }
                                    // pending removed — transfer complete
                                } else {
                                    // Unexpected frame; reinsert
                                    pending.state =
                                        SdoPendingState::DownloadSegmented { remaining, toggle };
                                    pending_sdos.insert(node_id, pending);
                                }
                            }
                        }
                    }
                } else {
                    // No pending for this node — passive decode (unchanged)
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

        // ── SDO timeout scan ─────────────────────────────────────────────────
        // CiA 301 abort code 0x05040000 = SDO protocol timed out.
        let timed_out: Vec<u8> = pending_sdos
            .iter()
            .filter(|(_, p)| p.started_at.elapsed().as_millis() as u64 >= sdo_timeout_ms)
            .map(|(id, _)| *id)
            .collect();

        for node_id in timed_out {
            if let Some(p) = pending_sdos.remove(&node_id) {
                let empty_od_inner = ObjectDictionary::new();
                let od_inner = find_od(ods, node_id).unwrap_or(&empty_od_inner);
                let name = od_inner
                    .get(&(p.index, p.subindex))
                    .map(|e| e.name.clone())
                    .unwrap_or_else(|| format!("{:04X}h/{:02X}", p.index, p.subindex));
                let entry = SdoLogEntry {
                    ts: Utc::now(),
                    node_id,
                    direction: p.direction,
                    index: p.index,
                    subindex: p.subindex,
                    name,
                    value: None,
                    abort_code: Some(0x0504_0000),
                };
                if tx.send(CanEvent::Sdo(entry)).is_err() {
                    return;
                }
            }
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
