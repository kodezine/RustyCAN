/// CAN session lifecycle: load EDS, open adapter, spawn recv thread.
///
/// Extracted from `main.rs` so the GUI can start/stop sessions without a CLI.
use std::collections::{HashMap, HashSet};
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
        calculate_crc16, decode_block_download_end_response,
        decode_block_download_initiate_response, decode_block_download_subblock_response,
        decode_block_upload_end, decode_block_upload_initiate_response,
        decode_block_upload_subblock, decode_sdo, decode_segmented_upload_initiate,
        decode_upload_segment_response, encode_block_download_end, encode_block_download_initiate,
        encode_block_download_subblock, encode_block_upload_end_response,
        encode_block_upload_initiate, encode_block_upload_response, encode_block_upload_start,
        encode_download_expedited, encode_download_initiate_segmented, encode_download_segment,
        encode_upload_request, encode_upload_segment_ack, interpret_value,
        is_download_initiate_ack, is_download_segment_ack, SdoDirection, SdoTransferMode,
    },
    FrameType,
};
use crate::eds::{parse_eds, types::ObjectDictionary};
use crate::logger::EventLogger;

// ─── Constants ─────────────────────────────────────────────────────────────────

/// Maximum buffer size for block transfers (10MB) to prevent DoS attacks
const MAX_BLOCK_TRANSFER_SIZE: usize = 10 * 1024 * 1024;

/// Data size threshold for using block vs segmented transfers (bytes)
const BLOCK_SIZE_THRESHOLD: usize = 64;

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
        /// Transfer mode: Auto (try block, fallback to segmented), ForcedSegmented, or ForcedBlock.
        mode: SdoTransferMode,
    },
    /// Initiate an SDO download (master writes to node). COB-ID 0x600+node_id.
    SdoWrite {
        node_id: u8,
        index: u16,
        subindex: u8,
        data: Vec<u8>,
        /// Transfer mode: Auto (try block, fallback to segmented), ForcedSegmented, or ForcedBlock.
        mode: SdoTransferMode,
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
    /// Timeout for block transfer initiate phase (milliseconds).
    pub block_initiate_timeout_ms: u64,
    /// Timeout for block transfer sub-block phase (milliseconds).
    pub block_subblock_timeout_ms: u64,
    /// Timeout for block transfer end phase (milliseconds).
    pub block_end_timeout_ms: u64,
    /// Default block size for block transfers (1-127 segments per block).
    pub block_size: u8,
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
    let block_initiate_timeout_ms = config.block_initiate_timeout_ms;
    let block_subblock_timeout_ms = config.block_subblock_timeout_ms;
    let block_end_timeout_ms = config.block_end_timeout_ms;
    let block_size = config.block_size;

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
            block_initiate_timeout_ms,
            block_subblock_timeout_ms,
            block_end_timeout_ms,
            block_size,
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
    _block_initiate_timeout_ms: u64,
    _block_subblock_timeout_ms: u64,
    _block_end_timeout_ms: u64,
    block_size: u8,
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
        /// Block download: waiting for initiate response from server.
        BlockDownloadInitiating { data: Vec<u8>, crc_enabled: bool },
        /// Block download: sending sub-block segments to server.
        BlockDownloadInProgress {
            remaining_data: Vec<u8>,
            seqno: u8,
            blksize: u8,
            crc: u16,
        },
        /// Block download: waiting for end confirmation from server.
        BlockDownloadEnding { crc_value: u16 },
        /// Block upload: waiting for initiate response from server.
        BlockUploadInitiating { blksize: u8, crc_enabled: bool },
        /// Block upload: receiving sub-block segments from server.
        BlockUploadInProgress {
            buffer: Vec<u8>,
            expected_seqno: u8,
            blksize: u8,
            crc: u16,
            crc_enabled: bool,
        },
        /// Block upload: waiting for end sequence from server.
        BlockUploadEnding {
            buffer: Vec<u8>,
            crc: u16,
            crc_enabled: bool,
        },
    }

    struct PendingSdo {
        #[allow(dead_code)]
        node_id: u8,
        index: u16,
        subindex: u8,
        direction: SdoDirection,
        started_at: Instant,
        last_activity: Instant,
        state: SdoPendingState,
    }

    let mut pending_sdos: HashMap<u8, PendingSdo> = HashMap::new();

    // Track nodes that don't support block transfers (for auto-fallback)
    let mut nodes_no_block: HashSet<u8> = HashSet::new();

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
                        mode,
                    } => {
                        // Decide whether to use block transfer
                        let use_block = match mode {
                            SdoTransferMode::ForcedBlock => true,
                            SdoTransferMode::ForcedSegmented => false,
                            SdoTransferMode::Auto => !nodes_no_block.contains(&node_id),
                        };

                        let cob_id = 0x600u16 + node_id as u16;

                        if use_block {
                            // Initiate block upload
                            let payload = encode_block_upload_initiate(
                                index, subindex, block_size,
                                0,    // pst = 0 (no protocol switch)
                                true, // CRC enabled
                            );
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
                                                last_activity: Instant::now(),
                                                state: SdoPendingState::BlockUploadInitiating {
                                                    blksize: block_size,
                                                    crc_enabled: true,
                                                },
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
                                        eprintln!("SDO block read send error: {e:?}");
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
                                            direction: SdoDirection::Read,
                                            index,
                                            subindex,
                                            name,
                                            value: None,
                                            abort_code: Some(0x08000000),
                                        }));
                                    }
                                }
                            }
                        } else {
                            // Use legacy segmented/expedited transfer
                            let payload = encode_upload_request(index, subindex);
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
                                                last_activity: Instant::now(),
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
                                            direction: SdoDirection::Read,
                                            index,
                                            subindex,
                                            name,
                                            value: None,
                                            abort_code: Some(0x08000000),
                                        }));
                                    }
                                }
                            }
                        }
                    }

                    CanCommand::SdoWrite {
                        node_id,
                        index,
                        subindex,
                        data,
                        mode,
                    } => {
                        let cob_id = 0x600u16 + node_id as u16;

                        if data.len() <= 4 {
                            // Expedited download (always use for small data)
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
                                                    last_activity: Instant::now(),
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
                                                abort_code: Some(0x08000000),
                                            }));
                                        }
                                    }
                                }
                            }
                        } else {
                            // Large data: decide between block and segmented
                            let use_block = match mode {
                                SdoTransferMode::ForcedBlock => true,
                                SdoTransferMode::ForcedSegmented => false,
                                SdoTransferMode::Auto => {
                                    // Use block for data > threshold if node supports it
                                    data.len() > BLOCK_SIZE_THRESHOLD
                                        && !nodes_no_block.contains(&node_id)
                                }
                            };

                            if use_block {
                                // Block download initiate
                                let size = data.len() as u32;
                                let payload = encode_block_download_initiate(
                                    index, subindex, size, true, // CRC enabled
                                );
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
                                                    last_activity: Instant::now(),
                                                    state:
                                                        SdoPendingState::BlockDownloadInitiating {
                                                            data,
                                                            crc_enabled: true,
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
                                            eprintln!("SDO block write send error: {e:?}");
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
                                                abort_code: Some(0x08000000),
                                            }));
                                        }
                                    }
                                }
                            } else {
                                // Segmented download initiate
                                let size = data.len() as u32;
                                let payload =
                                    encode_download_initiate_segmented(index, subindex, size);
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
                                                    last_activity: Instant::now(),
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
                                                abort_code: Some(0x08000000),
                                            }));
                                        }
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

                        // Check for block transfer not supported (auto-fallback)
                        if abort_code == 0x05040001 {
                            // Protocol not supported - check if we can fallback
                            let should_fallback = matches!(
                                pending.state,
                                SdoPendingState::BlockDownloadInitiating { .. }
                                    | SdoPendingState::BlockUploadInitiating { .. }
                            );

                            if should_fallback {
                                // Mark node as not supporting block transfers
                                nodes_no_block.insert(node_id);

                                // Re-initiate transfer with segmented mode
                                let cob_id = 0x600u16 + node_id as u16;
                                match pending.state {
                                    SdoPendingState::BlockDownloadInitiating {
                                        data: init_data,
                                        ..
                                    } => {
                                        // Retry with segmented download
                                        let size = init_data.len() as u32;
                                        let payload = encode_download_initiate_segmented(
                                            pending.index,
                                            pending.subindex,
                                            size,
                                        );
                                        if let Some(frame) = host_can::id::new_standard(cob_id)
                                            .and_then(|id| CanFrame::new(id, &payload))
                                        {
                                            if adapter.send(&frame).is_ok() {
                                                // Reinsert with segmented state
                                                pending.state =
                                                    SdoPendingState::DownloadSegmented {
                                                        remaining: init_data,
                                                        toggle: false,
                                                    };
                                                pending.started_at = Instant::now();
                                                pending.last_activity = Instant::now();
                                                pending_sdos.insert(node_id, pending);
                                                // Don't send abort event - transparent fallback
                                                continue;
                                            }
                                        }
                                    }
                                    SdoPendingState::BlockUploadInitiating { .. } => {
                                        // Retry with segmented/expedited upload
                                        let payload =
                                            encode_upload_request(pending.index, pending.subindex);
                                        if let Some(frame) = host_can::id::new_standard(cob_id)
                                            .and_then(|id| CanFrame::new(id, &payload))
                                        {
                                            if adapter.send(&frame).is_ok() {
                                                // Reinsert with waiting state
                                                pending.state = SdoPendingState::WaitingResponse;
                                                pending.started_at = Instant::now();
                                                pending.last_activity = Instant::now();
                                                pending_sdos.insert(node_id, pending);
                                                // Don't send abort event - transparent fallback
                                                continue;
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }

                        // Normal abort handling (if not fallback or fallback failed)
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

                            // ── Block Download States ──────────────────────────────
                            SdoPendingState::BlockDownloadInitiating {
                                data: init_data,
                                crc_enabled,
                            } => {
                                if let Some(blksize) = decode_block_download_initiate_response(data)
                                {
                                    // Server accepted block transfer, start sending sub-block
                                    let crc = calculate_crc16(&init_data);
                                    let mut remaining_data = init_data;
                                    let mut seqno = 1u8;

                                    // Send first segment
                                    let chunk_len = remaining_data.len().min(7);
                                    let chunk = remaining_data[..chunk_len].to_vec();
                                    remaining_data = remaining_data[chunk_len..].to_vec();

                                    let seg_frame =
                                        encode_block_download_subblock(seqno, &chunk, false);
                                    let cob_out = 0x600u16 + node_id as u16;
                                    if let Some(f) = host_can::id::new_standard(cob_out)
                                        .and_then(|id| CanFrame::new(id, &seg_frame))
                                    {
                                        if let Err(e) = adapter.send(&f) {
                                            eprintln!("Block download segment send error: {e:?}");
                                            // Abort transfer - send error occurred
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
                                                abort_code: Some(0x08000000), // General error
                                            };
                                            if tx.send(CanEvent::Sdo(entry)).is_err() {
                                                return;
                                            }
                                            continue;
                                        }
                                    }
                                    seqno += 1;
                                    // Wrap sequence number: 1-127 range per CiA 301
                                    if seqno > 127 {
                                        seqno = 1;
                                    }

                                    pending.state = SdoPendingState::BlockDownloadInProgress {
                                        remaining_data,
                                        seqno,
                                        blksize,
                                        crc,
                                    };
                                    pending.last_activity = Instant::now();
                                    pending_sdos.insert(node_id, pending);
                                } else {
                                    // Unexpected response; reinsert for timeout
                                    pending.state = SdoPendingState::BlockDownloadInitiating {
                                        data: init_data,
                                        crc_enabled,
                                    };
                                    pending_sdos.insert(node_id, pending);
                                }
                            }

                            SdoPendingState::BlockDownloadInProgress {
                                mut remaining_data,
                                mut seqno,
                                blksize,
                                crc,
                            } => {
                                if let Some((_ackseq, new_blksize)) =
                                    decode_block_download_subblock_response(data)
                                {
                                    // Server acknowledged sub-block
                                    pending.last_activity = Instant::now();

                                    if remaining_data.is_empty() {
                                        // All data sent, send end sequence
                                        let n = 0; // No unused bytes in last segment
                                        let end_frame = encode_block_download_end(n, crc);
                                        let cob_out = 0x600u16 + node_id as u16;
                                        if let Some(f) = host_can::id::new_standard(cob_out)
                                            .and_then(|id| CanFrame::new(id, &end_frame))
                                        {
                                            let _ = adapter.send(&f);
                                        }
                                        pending.state =
                                            SdoPendingState::BlockDownloadEnding { crc_value: crc };
                                        pending_sdos.insert(node_id, pending);
                                    } else {
                                        // Continue sending next sub-block
                                        seqno = 1; // Reset sequence for new sub-block
                                        let segments_to_send = new_blksize.min(127);

                                        for _ in 0..segments_to_send {
                                            if remaining_data.is_empty() {
                                                break;
                                            }
                                            let chunk_len = remaining_data.len().min(7);
                                            let chunk = remaining_data[..chunk_len].to_vec();
                                            remaining_data = remaining_data[chunk_len..].to_vec();

                                            let seg_frame = encode_block_download_subblock(
                                                seqno, &chunk, false,
                                            );
                                            let cob_out = 0x600u16 + node_id as u16;
                                            if let Some(f) = host_can::id::new_standard(cob_out)
                                                .and_then(|id| CanFrame::new(id, &seg_frame))
                                            {
                                                if let Err(e) = adapter.send(&f) {
                                                    eprintln!("Block download sub-block segment send error: {e:?}");
                                                    // Continue with remaining data - will timeout if persistent
                                                }
                                            }
                                            seqno += 1;
                                            // Wrap sequence number: 1-127 range per CiA 301
                                            if seqno > 127 {
                                                seqno = 1;
                                            }
                                        }

                                        pending.state = SdoPendingState::BlockDownloadInProgress {
                                            remaining_data,
                                            seqno,
                                            blksize: new_blksize,
                                            crc,
                                        };
                                        pending_sdos.insert(node_id, pending);
                                    }
                                } else {
                                    // Unexpected response; reinsert for timeout
                                    pending.state = SdoPendingState::BlockDownloadInProgress {
                                        remaining_data,
                                        seqno,
                                        blksize,
                                        crc,
                                    };
                                    pending_sdos.insert(node_id, pending);
                                }
                            }

                            SdoPendingState::BlockDownloadEnding { crc_value } => {
                                if decode_block_download_end_response(data) {
                                    // Transfer complete
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
                                    // pending removed
                                } else {
                                    // Unexpected response; reinsert for timeout
                                    pending.state =
                                        SdoPendingState::BlockDownloadEnding { crc_value };
                                    pending_sdos.insert(node_id, pending);
                                }
                            }

                            // ── Block Upload States ────────────────────────────────
                            SdoPendingState::BlockUploadInitiating {
                                blksize,
                                crc_enabled,
                            } => {
                                if let Some((server_crc_enabled, _size)) =
                                    decode_block_upload_initiate_response(data)
                                {
                                    // Server accepted, send start command
                                    pending.last_activity = Instant::now();
                                    let start_frame = encode_block_upload_start();
                                    let cob_out = 0x600u16 + node_id as u16;
                                    if let Some(f) = host_can::id::new_standard(cob_out)
                                        .and_then(|id| CanFrame::new(id, &start_frame))
                                    {
                                        let _ = adapter.send(&f);
                                    }

                                    pending.state = SdoPendingState::BlockUploadInProgress {
                                        buffer: Vec::new(),
                                        expected_seqno: 1,
                                        blksize,
                                        crc: 0,
                                        crc_enabled: server_crc_enabled,
                                    };
                                    pending_sdos.insert(node_id, pending);
                                } else {
                                    // Unexpected response; reinsert for timeout
                                    pending.state = SdoPendingState::BlockUploadInitiating {
                                        blksize,
                                        crc_enabled,
                                    };
                                    pending_sdos.insert(node_id, pending);
                                }
                            }

                            SdoPendingState::BlockUploadInProgress {
                                mut buffer,
                                mut expected_seqno,
                                blksize,
                                crc,
                                crc_enabled,
                            } => {
                                if let Some((seqno, payload, is_last)) =
                                    decode_block_upload_subblock(data)
                                {
                                    if seqno == expected_seqno {
                                        // Correct sequence, accumulate data
                                        pending.last_activity = Instant::now();

                                        // Check buffer size limit to prevent DoS
                                        if buffer.len() + payload.len() > MAX_BLOCK_TRANSFER_SIZE {
                                            // Abort: data too large
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
                                                value: None,
                                                abort_code: Some(0x05040005), // Out of memory
                                            };
                                            if tx.send(CanEvent::Sdo(entry)).is_err() {
                                                return;
                                            }
                                            continue;
                                        }

                                        buffer.extend_from_slice(&payload);

                                        if is_last || expected_seqno >= blksize {
                                            // End of sub-block, send acknowledgment
                                            let ack_frame =
                                                encode_block_upload_response(seqno, blksize);
                                            let cob_out = 0x600u16 + node_id as u16;
                                            if let Some(f) = host_can::id::new_standard(cob_out)
                                                .and_then(|id| CanFrame::new(id, &ack_frame))
                                            {
                                                if let Err(e) = adapter.send(&f) {
                                                    eprintln!("Block upload ack send error: {e:?}");
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
                                                        value: None,
                                                        abort_code: Some(0x08000000), // General error
                                                    };
                                                    if tx.send(CanEvent::Sdo(entry)).is_err() {
                                                        return;
                                                    }
                                                    continue;
                                                }
                                            }

                                            if is_last {
                                                // Transition to ending state
                                                let calculated_crc = calculate_crc16(&buffer);
                                                pending.state =
                                                    SdoPendingState::BlockUploadEnding {
                                                        buffer,
                                                        crc: calculated_crc,
                                                        crc_enabled,
                                                    };
                                            } else {
                                                // Continue to next sub-block
                                                expected_seqno = 1;
                                                pending.state =
                                                    SdoPendingState::BlockUploadInProgress {
                                                        buffer,
                                                        expected_seqno,
                                                        blksize,
                                                        crc,
                                                        crc_enabled,
                                                    };
                                            }
                                            pending_sdos.insert(node_id, pending);
                                        } else {
                                            // Continue receiving segments
                                            expected_seqno += 1;
                                            // Wrap sequence number: 1-127 range per CiA 301
                                            if expected_seqno > 127 {
                                                expected_seqno = 1;
                                            }
                                            pending.state =
                                                SdoPendingState::BlockUploadInProgress {
                                                    buffer,
                                                    expected_seqno,
                                                    blksize,
                                                    crc,
                                                    crc_enabled,
                                                };
                                            pending_sdos.insert(node_id, pending);
                                        }
                                    } else {
                                        // Sequence error - abort transfer
                                        eprintln!("Block upload sequence error: expected {expected_seqno}, got {seqno}");
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
                                            value: None,
                                            abort_code: Some(0x05040003), // Invalid sequence number
                                        };
                                        if tx.send(CanEvent::Sdo(entry)).is_err() {
                                            return;
                                        }
                                        // pending removed - transfer aborted
                                    }
                                } else {
                                    // Unexpected frame; reinsert for timeout
                                    pending.state = SdoPendingState::BlockUploadInProgress {
                                        buffer,
                                        expected_seqno,
                                        blksize,
                                        crc,
                                        crc_enabled,
                                    };
                                    pending_sdos.insert(node_id, pending);
                                }
                            }

                            SdoPendingState::BlockUploadEnding {
                                buffer,
                                crc,
                                crc_enabled,
                            } => {
                                if let Some((n, server_crc)) = decode_block_upload_end(data) {
                                    // Validate CRC if enabled
                                    let crc_valid = !crc_enabled || crc == server_crc;

                                    if !crc_valid {
                                        eprintln!(
                                            "Block upload CRC mismatch: computed {:#06X}, server sent {:#06X}",
                                            crc, server_crc
                                        );
                                    }

                                    if crc_valid {
                                        // Remove unused bytes from end
                                        let final_len = buffer.len().saturating_sub(n as usize);
                                        let final_data = &buffer[..final_len];

                                        // Send end acknowledgment
                                        let end_ack = encode_block_upload_end_response();
                                        let cob_out = 0x600u16 + node_id as u16;
                                        if let Some(f) = host_can::id::new_standard(cob_out)
                                            .and_then(|id| CanFrame::new(id, &end_ack))
                                        {
                                            let _ = adapter.send(&f);
                                        }

                                        // Decode value and emit success
                                        let opt_dtype = od
                                            .get(&(pending.index, pending.subindex))
                                            .map(|e| &e.data_type);
                                        let value = interpret_value(final_data, opt_dtype);
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
                                        // pending removed
                                    } else {
                                        // CRC mismatch, abort
                                        let abort_code = 0x05040004u32; // CRC error
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
                                            value: None,
                                            abort_code: Some(abort_code),
                                        };
                                        if tx.send(CanEvent::Sdo(entry)).is_err() {
                                            return;
                                        }
                                        // pending removed
                                    }
                                } else {
                                    // Unexpected response; reinsert for timeout
                                    pending.state = SdoPendingState::BlockUploadEnding {
                                        buffer,
                                        crc,
                                        crc_enabled,
                                    };
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
        // Use per-stage timeouts for block transfers
        let timed_out: Vec<u8> = pending_sdos
            .iter()
            .filter(|(_, p)| {
                let timeout_ms = match &p.state {
                    SdoPendingState::BlockDownloadInitiating { .. }
                    | SdoPendingState::BlockUploadInitiating { .. } => _block_initiate_timeout_ms,
                    SdoPendingState::BlockDownloadInProgress { .. }
                    | SdoPendingState::BlockUploadInProgress { .. } => {
                        // Use last_activity for in-progress stages
                        return p.last_activity.elapsed().as_millis() as u64
                            >= _block_subblock_timeout_ms;
                    }
                    SdoPendingState::BlockDownloadEnding { .. }
                    | SdoPendingState::BlockUploadEnding { .. } => _block_end_timeout_ms,
                    _ => sdo_timeout_ms,
                };
                p.started_at.elapsed().as_millis() as u64 >= timeout_ms
            })
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
