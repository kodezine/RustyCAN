use rustycan::canopen;
use rustycan::eds;
use rustycan::logger;
use rustycan::tui;

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use chrono::Utc;
use clap::Parser;

use canopen::{
    classify_frame, extract_cob_id,
    nmt::{decode_heartbeat, decode_nmt_command},
    pdo::PdoDecoder,
    sdo::decode_sdo,
    FrameType,
};
use eds::parse_eds;
use eds::types::ObjectDictionary;
use logger::EventLogger;
use tui::{AppState, CanEvent, SdoLogEntry};

// ─── CLI ─────────────────────────────────────────────────────────────────────

/// RustyCAN — CANopen viewer for macOS (PEAK PCAN-USB)
#[derive(Parser, Debug)]
#[command(
    name = "rustycan",
    about = "Log and analyze SDO/PDO/NMT events from multiple CANopen devices",
    long_about = None,
)]
struct Cli {
    /// PCAN-USB channel number (1–8, corresponds to PCAN_USBBUS<N>).
    #[arg(long, default_value = "1")]
    port: String,

    /// CAN bus baud rate in bps (e.g. 250000, 500000, 1000000).
    #[arg(long, default_value = "250000")]
    baud: u32,

    /// Map a Node-ID to an EDS file.  Repeat for multiple nodes.
    /// Format: `<node_id>:<path/to/device.eds>`
    /// Example: `--node 1:motor.eds --node 2:sensor.eds`
    #[arg(long = "node", value_name = "ID:PATH")]
    nodes: Vec<String>,

    /// Path of the JSONL event log file.
    #[arg(long, default_value = "rustycan.jsonl")]
    log: String,
}

fn parse_node_arg(s: &str) -> Result<(u8, PathBuf), String> {
    let (id_str, path_str) = s
        .split_once(':')
        .ok_or_else(|| format!("expected <ID>:<PATH>, got {s:?}"))?;
    let id: u8 = id_str
        .trim()
        .parse()
        .map_err(|_| format!("invalid node-id {id_str:?}"))?;
    Ok((id, PathBuf::from(path_str.trim())))
}

// ─── Main ─────────────────────────────────────────────────────────────────────

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Parse --node arguments.
    let node_specs: Vec<(u8, PathBuf)> = cli
        .nodes
        .iter()
        .map(|s| {
            parse_node_arg(s).unwrap_or_else(|e| {
                eprintln!("Error: {e}");
                std::process::exit(1);
            })
        })
        .collect();

    // Load EDS files and build per-node state.
    let mut node_ods: Vec<(u8, ObjectDictionary)> = Vec::new();
    let mut node_labels: Vec<(u8, String)> = Vec::new();

    for (node_id, eds_path) in &node_specs {
        let label = eds_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("node{node_id}.eds"));

        let od = parse_eds(eds_path).unwrap_or_else(|e| {
            eprintln!("Failed to load EDS {}: {e}", eds_path.display());
            std::process::exit(1);
        });

        println!(
            "Loaded EDS for node {node_id} ({label}): {} OD entries",
            od.len()
        );
        node_labels.push((*node_id, label));
        node_ods.push((*node_id, od));
    }

    // Build PDO decoders per node.
    let pdo_decoders: Vec<(u8, PdoDecoder)> = node_ods
        .iter()
        .map(|(id, od)| (*id, PdoDecoder::from_od(*id, od)))
        .collect();

    // Open logger.
    let logger = EventLogger::new(&cli.log).unwrap_or_else(|e| {
        eprintln!("Failed to open log file {}: {e}", cli.log);
        std::process::exit(1);
    });

    println!("Starting capture → {}", cli.log);

    // ── spawn recv thread ──────────────────────────────────────────────────
    // The adapter is opened inside the thread to avoid the `Send` bound on
    // `dyn Adapter` (host-can does not guarantee thread safety on the trait
    // object; opening on the same OS thread that will use it is safe).
    let (tx, rx) = mpsc::channel::<CanEvent>();

    let port = cli.port.clone();
    let baud = cli.baud;
    let ods_for_thread: Vec<(u8, ObjectDictionary)> = node_ods;
    let pdo_for_thread: Vec<(u8, PdoDecoder)> = pdo_decoders;

    thread::spawn(move || {
        let adapter = host_can::adapter::get_adapter(&port, baud).unwrap_or_else(|e| {
            eprintln!(
                "\nFailed to open PCAN-USB channel {port}: {e}\n\
                     Make sure the PCUSB library is installed from https://mac-can.com\n\
                     and the adapter is connected.\n"
            );
            std::process::exit(1);
        });
        eprintln!("Adapter open on channel {port}.");
        recv_loop(adapter, &ods_for_thread, &pdo_for_thread, tx, logger);
    });

    // ── run TUI ───────────────────────────────────────────────────────────
    let state = AppState::new(cli.log.clone());
    tui::run(rx, state, node_labels)?;

    Ok(())
}

// ─── Receive loop (runs on a dedicated thread) ────────────────────────────────

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
                // ReadTimeout is normal; anything else is a real error.
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
                    // Heartbeat / command events don't carry a specific node_id here;
                    // emit separate state updates if target_node is specified.
                    if let canopen::nmt::NmtEvent::Command {
                        command: _,
                        target_node,
                    } = &ev
                    {
                        // If the command targets a specific node, update its state.
                        // We don't know the new state from a command alone; the
                        // node's next heartbeat will confirm it.
                        let _ = target_node; // used via logger above
                    }
                }
            }

            // ── NMT heartbeat / bootup ────────────────────────────────────
            FrameType::Heartbeat(node_id) => {
                if let Some(ev) = decode_heartbeat(node_id, data) {
                    logger.log_nmt(ts, &ev);
                    if let canopen::nmt::NmtEvent::Heartbeat { node_id, ref state } = ev {
                        let _ = tx.send(CanEvent::Nmt {
                            node_id,
                            state: state.clone(),
                        });
                    }
                }
            }

            // ── SDO response (device → master) ─────────────────────────────
            FrameType::SdoResponse(node_id) => {
                let od = find_od(ods, node_id);
                if let Some(sdo_ev) = decode_sdo(node_id, data, od, true) {
                    logger.log_sdo(ts, &sdo_ev, data);
                    let _ = tx.send(CanEvent::Sdo(SdoLogEntry {
                        ts,
                        node_id: sdo_ev.node_id,
                        direction: sdo_ev.direction,
                        index: sdo_ev.index,
                        subindex: sdo_ev.subindex,
                        name: sdo_ev.name,
                        value: sdo_ev.value,
                        abort_code: sdo_ev.abort_code,
                    }));
                }
            }

            // ── SDO request (master → device) ─────────────────────────────
            FrameType::SdoRequest(node_id) => {
                let od = find_od(ods, node_id);
                if let Some(sdo_ev) = decode_sdo(node_id, data, od, false) {
                    logger.log_sdo(ts, &sdo_ev, data);
                    let _ = tx.send(CanEvent::Sdo(SdoLogEntry {
                        ts,
                        node_id: sdo_ev.node_id,
                        direction: sdo_ev.direction,
                        index: sdo_ev.index,
                        subindex: sdo_ev.subindex,
                        name: sdo_ev.name,
                        value: sdo_ev.value,
                        abort_code: sdo_ev.abort_code,
                    }));
                }
            }

            // ── TPDO ─────────────────────────────────────────────────────
            FrameType::Tpdo(pdo_num, node_id) => {
                let decoder = find_pdo_decoder(pdo_decoders, node_id);
                if let Some(values) = decoder.and_then(|d| d.decode(cob_id, data)) {
                    logger.log_pdo(ts, node_id, pdo_num, &values, data);
                    let _ = tx.send(CanEvent::Pdo {
                        node_id,
                        pdo_num,
                        values,
                    });
                }
            }

            // ── RPDO (master → device; log only) ─────────────────────────
            FrameType::Rpdo(pdo_num, node_id) => {
                let decoder = find_pdo_decoder(pdo_decoders, node_id);
                if let Some(values) = decoder.and_then(|d| d.decode(cob_id, data)) {
                    logger.log_pdo(ts, node_id, pdo_num, &values, data);
                    // RPDO values are sent TO the device, not shown in the live
                    // PDO panel by default, but could be added if desired.
                }
            }

            // Ignore SYNC, EMCY, and unknown frames.
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
            // Return a reference to the first OD (empty fallback) or panic.
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
