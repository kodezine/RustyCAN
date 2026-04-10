use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::{DateTime, Local, Utc};
use serde_json::{json, Value};

use crate::canopen::nmt::{NmtCommand, NmtEvent, NmtState};
use crate::canopen::pdo::{PdoRawValue, PdoValue};
use crate::canopen::sdo::{SdoDirection, SdoEvent, SdoValue};

/// Appends newline-delimited JSON log entries to a file.
///
/// Uses buffered writes with periodic flushing to avoid blocking the
/// CAN receive loop in high-traffic scenarios (7-10+ nodes).
pub struct EventLogger {
    writer: BufWriter<File>,
    /// Optional plain-text companion log (`.log` file, same timestamp stem).
    text_writer: Option<BufWriter<File>>,
    /// Count of log entries since last flush.
    entries_since_flush: usize,
    /// Time of last flush.
    last_flush: Instant,
    /// Flush every N entries.
    flush_interval_entries: usize,
    /// Flush at least every N milliseconds.
    flush_interval_ms: u64,
    /// Hardware timestamp (µs since bus-on) from the KCAN dongle ISR.
    ///
    /// Set by [`set_hw_timestamp`][Self::set_hw_timestamp] before each frame's
    /// log call; cleared after each [`log`][Self::log] so it is only included
    /// in the entry it belongs to.  `None` for PEAK (no hardware timestamps).
    hw_timestamp_us: Option<u32>,
}

impl EventLogger {
    pub fn new<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        Self::with_config(path, 50, 100)
    }

    /// Create a logger that also writes a parallel plain-text `.log` file.
    ///
    /// When `text_log` is `false` this is identical to [`new`][Self::new].
    /// When `true`, a second file is opened alongside the JSONL file with the
    /// same timestamped stem but a `.log` extension.  Both files share the same
    /// flush cadence so there is no additional performance overhead.
    pub fn with_text_log<P: AsRef<Path>>(path: P, text_log: bool) -> std::io::Result<Self> {
        let timestamped_path = add_timestamp_to_path(path.as_ref());
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&timestamped_path)?;
        let writer = BufWriter::with_capacity(64 * 1024, file);

        let text_writer = if text_log {
            let text_path = timestamped_path.with_extension("log");
            let text_file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&text_path)?;
            Some(BufWriter::with_capacity(64 * 1024, text_file))
        } else {
            None
        };

        Ok(Self {
            writer,
            text_writer,
            entries_since_flush: 0,
            last_flush: Instant::now(),
            flush_interval_entries: 50,
            flush_interval_ms: 100,
            hw_timestamp_us: None,
        })
    }

    /// Create a logger with custom flush intervals.
    ///
    /// # Arguments
    /// * `flush_interval_entries` - Flush after this many log entries
    /// * `flush_interval_ms` - Flush at least every this many milliseconds
    ///
    /// For high-traffic scenarios (10+ nodes), consider:
    /// - `flush_interval_entries`: 100-200 (default: 50)
    /// - `flush_interval_ms`: 200-500 (default: 100)
    pub fn with_config<P: AsRef<Path>>(
        path: P,
        flush_interval_entries: usize,
        flush_interval_ms: u64,
    ) -> std::io::Result<Self> {
        let timestamped_path = add_timestamp_to_path(path.as_ref());
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&timestamped_path)?;

        // Use a larger buffer for high-traffic scenarios (default is 8KB)
        let writer = BufWriter::with_capacity(64 * 1024, file);

        Ok(Self {
            writer,
            text_writer: None,
            entries_since_flush: 0,
            last_flush: Instant::now(),
            flush_interval_entries,
            flush_interval_ms,
            hw_timestamp_us: None,
        })
    }

    /// Set the hardware timestamp for the **next** [`log`][Self::log] call.
    ///
    /// The value is consumed (reset to `None`) after the call to `log()` so
    /// it is never accidentally attached to a subsequent unrelated entry.
    ///
    /// Call this immediately before each frame's log method:
    /// ```ignore
    /// logger.set_hw_timestamp(hardware_timestamp_us);
    /// logger.log_nmt(ts, &ev, data, cob_id);
    /// ```
    pub fn set_hw_timestamp(&mut self, ts: Option<u32>) {
        self.hw_timestamp_us = ts;
    }

    /// Write a pre-built JSON `Value` as a single log line.
    /// Flushes periodically to balance data safety with performance.
    pub fn log(&mut self, mut entry: Value) {
        // Attach hardware timestamp if one was set for this frame.
        if let Some(hw_ts) = self.hw_timestamp_us.take() {
            if let Value::Object(ref mut map) = entry {
                map.insert("hw_ts_us".to_string(), serde_json::json!(hw_ts));
            }
        }

        if let Ok(s) = serde_json::to_string(&entry) {
            // Write the entry (buffered)
            if writeln!(self.writer, "{s}").is_err() {
                // Silently skip this entry if write fails
                return;
            }

            self.entries_since_flush += 1;

            // Flush if either condition is met:
            // 1. Reached the entry count threshold
            // 2. Enough time has passed since last flush
            let should_flush = self.entries_since_flush >= self.flush_interval_entries
                || self.last_flush.elapsed().as_millis() as u64 >= self.flush_interval_ms;

            if should_flush {
                let _ = self.writer.flush();
                if let Some(w) = &mut self.text_writer {
                    let _ = w.flush();
                }
                self.entries_since_flush = 0;
                self.last_flush = Instant::now();
            }
        }
    }

    /// Force an immediate flush to disk.
    /// Use sparingly (e.g., after important SDO operations) to ensure
    /// critical events are persisted even if the process crashes.
    pub fn force_flush(&mut self) {
        let _ = self.writer.flush();
        if let Some(w) = &mut self.text_writer {
            let _ = w.flush();
        }
        self.entries_since_flush = 0;
        self.last_flush = Instant::now();
    }

    /// Write a `session_start` header as the very first entry in the log.
    ///
    /// Records the adapter name, baud rate, and host workstation metadata so
    /// every log file is self-describing. On macOS the metadata is gathered
    /// from `sw_vers` and `sysctl`; degrades gracefully on other platforms.
    pub fn log_session_start(&mut self, ts: DateTime<Utc>, adapter_name: &str, baud: u32) {
        let host = collect_host_info();
        let ts_str = ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let entry = json!({
            "ts":      ts_str,
            "type":    "session_start",
            "adapter": adapter_name,
            "baud":    baud,
            "host":    host,
        });
        // Plain-text header line — written directly, not via write_text_line
        // which expects a CAN-frame-formatted row.
        if let Some(w) = &mut self.text_writer {
            let ts_local = ts
                .with_timezone(&Local)
                .format("%Y-%m-%dT%H:%M:%S%.3f%z")
                .to_string();
            let os = host["os"].as_str().unwrap_or("-");
            let model = host["model"].as_str().unwrap_or("-");
            let _ = writeln!(
                w,
                "[{ts_local}][session_start    ][---------] adapter=\"{adapter_name}\" baud={baud} os=\"{os}\" model=\"{model}\""
            );
        }
        self.log(entry);
    }

    /// Log an NMT master command that was sent by this application.
    pub fn log_nmt_sent(
        &mut self,
        ts: DateTime<Utc>,
        command: &NmtCommand,
        target_node: u8,
        raw: &[u8],
    ) {
        let ts_str = ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let entry = json!({
            "ts": ts_str,
            "type": "NMT_COMMAND_SENT",
            "cob_id": "0x000",
            "source_node": 0,
            "command": format_nmt_command(command),
            "target_node": target_node,
            "raw": bytes_to_hex(raw),
        });
        self.log(entry);
        self.write_text_line(&ts_str, "NMT_COMMAND_SENT", "0x000", raw);
    }

    pub fn log_nmt(&mut self, ts: DateTime<Utc>, event: &NmtEvent, raw: &[u8], cob_id: u16) {
        let ts_str = ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let cob_id_str = format!("0x{cob_id:03X}");
        let (entry, type_str) = match event {
            NmtEvent::Command {
                command,
                target_node,
            } => (
                json!({
                    "ts": ts_str,
                    "type": "NMT_COMMAND",
                    "cob_id": cob_id_str,
                    // NMT commands on COB-ID 0x000 always originate from the master.
                    "source_node": 0,
                    "command": format_nmt_command(command),
                    "target_node": target_node,
                    "raw": bytes_to_hex(raw),
                }),
                "NMT_COMMAND",
            ),
            NmtEvent::Heartbeat { node_id, state } => (
                json!({
                    "ts": ts_str,
                    "type": "NMT_STATE",
                    "cob_id": cob_id_str,
                    // Heartbeat / error-control frames originate from the node itself.
                    "source_node": node_id,
                    "node": node_id,
                    "state": format_nmt_state(state),
                    "raw": bytes_to_hex(raw),
                }),
                "NMT_STATE",
            ),
        };
        self.log(entry);
        self.write_text_line(&ts_str, type_str, &cob_id_str, raw);
    }

    pub fn log_sdo(&mut self, ts: DateTime<Utc>, event: &SdoEvent, raw: &[u8], cob_id: u16) {
        let ts_str = ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let direction = match event.direction {
            SdoDirection::Read => "READ",
            SdoDirection::Write => "WRITE",
        };
        let type_str = format!("SDO_{direction}");
        let cob_id_str = format!("0x{cob_id:03X}");
        // COB-ID 0x580–0x5FF: server (node) response; 0x600–0x67F: client (master) request.
        let source_node: u8 = if (0x580..=0x5FF).contains(&cob_id) {
            event.node_id
        } else {
            0
        };
        let mut entry = json!({
            "ts": ts_str,
            "type": type_str,
            "cob_id": cob_id_str,
            "source_node": source_node,
            "node": event.node_id,
            "index": format!("0x{:04X}", event.index),
            "subindex": format!("0x{:02X}", event.subindex),
            "name": event.name,
            "raw": bytes_to_hex(raw),
        });
        if let Some(v) = &event.value {
            entry["value"] = serde_json::to_value(v).unwrap_or(Value::Null);

            // For byte arrays, add ASCII representation if printable
            if let SdoValue::Bytes(bytes) = v {
                if bytes.iter().all(|&b| {
                    b == 0 || b == 0x09 || b == 0x0A || b == 0x0D || (0x20..0x7F).contains(&b)
                }) {
                    // Strip trailing null bytes for ASCII display
                    let trimmed: Vec<u8> = bytes.iter().take_while(|&&b| b != 0).copied().collect();
                    if let Ok(ascii_str) = String::from_utf8(trimmed) {
                        entry["ascii"] = json!(ascii_str);
                    }
                }
            }
        }
        if let Some(code) = event.abort_code {
            entry["abort_code"] = json!(format!("0x{code:08X}"));
        }
        self.log(entry);
        self.write_text_line(&ts_str, &type_str, &cob_id_str, raw);
    }

    pub fn log_pdo(
        &mut self,
        ts: DateTime<Utc>,
        node_id: u8,
        pdo_num: u8,
        values: &[PdoValue],
        raw: &[u8],
        cob_id: u16,
    ) {
        let ts_str = ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let cob_id_str = format!("0x{cob_id:03X}");
        let mut signals = serde_json::Map::new();
        for v in values {
            signals.insert(v.signal_name.clone(), pdo_value_to_json(&v.value));
        }

        let entry = json!({
            "ts": ts_str,
            "type": "PDO",
            "cob_id": cob_id_str,
            // TPDOs are transmitted by the node itself.
            "source_node": node_id,
            "node": node_id,
            "pdo_num": pdo_num,
            "signals": signals,
            "raw": bytes_to_hex(raw),
        });
        self.log(entry);
        self.write_text_line(&ts_str, "PDO", &cob_id_str, raw);
    }

    /// Log a DBC-decoded signal frame.
    pub fn log_dbc_signal(
        &mut self,
        ts: DateTime<Utc>,
        frame_signals: &crate::dbc::types::DbcFrameSignals,
        raw: &[u8],
        cob_id: u16,
    ) {
        let ts_str = ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let cob_id_str = format!("0x{cob_id:03X}");

        // Build signals map: signal_name → {raw, physical, unit, description}
        let mut signals = serde_json::Map::new();
        for sig in &frame_signals.values {
            let mut sig_obj = serde_json::Map::new();
            sig_obj.insert("raw".to_string(), json!(sig.raw_int));
            sig_obj.insert("physical".to_string(), json!(sig.physical));
            sig_obj.insert("unit".to_string(), json!(sig.unit));
            sig_obj.insert(
                "description".to_string(),
                sig.description
                    .as_ref()
                    .map(|s| json!(s))
                    .unwrap_or(json!(null)),
            );
            signals.insert(sig.signal_name.clone(), json!(sig_obj));
        }

        let entry = json!({
            "ts": ts_str,
            "type": "DBC_SIGNAL",
            "cob_id": cob_id_str,
            "source_node": json!(null), // DBC frames don't infer source from COB-ID
            "message": frame_signals.message_name,
            "source_dbc": frame_signals.source_dbc,
            "signals": signals,
            "raw": bytes_to_hex(raw),
        });
        self.log(entry);
        self.write_text_line(&ts_str, "DBC_SIGNAL", &cob_id_str, raw);
    }

    /// Log a raw CAN frame (fallback for frames not decoded by DBC or CANopen).
    pub fn log_raw_frame(&mut self, ts: DateTime<Utc>, can_id: u16, raw: &[u8]) {
        let ts_str = ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let cob_id_str = format!("0x{can_id:03X}");

        let entry = json!({
            "ts": ts_str,
            "type": "RAW_FRAME",
            "cob_id": cob_id_str,
            "source_node": json!(null),
            "raw": bytes_to_hex(raw),
        });
        self.log(entry);
        self.write_text_line(&ts_str, "RAW_FRAME", &cob_id_str, raw);
    }
}

impl Drop for EventLogger {
    fn drop(&mut self) {
        let _ = self.writer.flush();
        if let Some(w) = &mut self.text_writer {
            let _ = w.flush();
        }
    }
}

// ─── Host metadata ────────────────────────────────────────────────────────────

/// Collect workstation information for the `session_start` log entry.
///
/// Uses macOS CLI tools (`sw_vers`, `sysctl`) where available; degrades
/// gracefully to `std::env::consts` on other platforms.
fn collect_host_info() -> Value {
    fn cmd_output(prog: &str, args: &[&str]) -> Option<String> {
        std::process::Command::new(prog)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    }

    // OS version — e.g. "macOS 15.3.2 (Build 24D81)"
    let os_name = cmd_output("sw_vers", &["-productName"]);
    let os_ver = cmd_output("sw_vers", &["-productVersion"]);
    let os_build = cmd_output("sw_vers", &["-buildVersion"]);
    let os = match (os_name, os_ver, os_build) {
        (Some(n), Some(v), Some(b)) => format!("{n} {v} (Build {b})"),
        (Some(n), Some(v), None) => format!("{n} {v}"),
        _ => std::env::consts::OS.to_string(),
    };

    // Hardware model identifier — e.g. "Mac14,3"
    let model = cmd_output("sysctl", &["-n", "hw.model"])
        .unwrap_or_else(|| std::env::consts::ARCH.to_string());

    // Hostname
    let hostname = cmd_output("hostname", &[])
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_else(|| "-".to_string());

    // Current user
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "-".to_string());

    json!({
        "os":       os,
        "model":    model,
        "arch":     std::env::consts::ARCH,
        "hostname": hostname,
        "user":     user,
    })
}

// ─── Private helpers ───────────────────────────────────────────────────────────

impl EventLogger {
    /// Write one line to the plain-text log.
    ///
    /// Format: `[<iso8601>][<type padded to 16>][<cob_id>] HH HH HH …`
    ///
    /// Called immediately after `self.log(entry)` in each public logging method.
    /// The write is buffered and flushed on the same cadence as the JSONL writer.
    fn write_text_line(&mut self, ts: &str, type_str: &str, cob_id: &str, raw: &[u8]) {
        if let Some(w) = &mut self.text_writer {
            let bytes_hex = raw
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ");
            // 16 chars covers the longest type ("NMT_COMMAND_SENT")
            let _ = writeln!(w, "[{ts}][{type_str:<16}][{cob_id}] {bytes_hex}");
        }
    }
}

/// Add a timestamp to a file path before the extension.
/// Example: "log.jsonl" -> "log_20263003130458.jsonl"
///
/// This function is public so `session::start` can use it to report the actual
/// filename that was created.
pub fn add_timestamp_to_path(path: &Path) -> PathBuf {
    let timestamp = Local::now().format("%Y%m%d%H%M%S").to_string();

    if let Some(stem) = path.file_stem() {
        let stem_str = stem.to_string_lossy();
        if let Some(ext) = path.extension() {
            // Has extension: insert timestamp before extension
            let new_name = format!("{}_{}.{}", stem_str, timestamp, ext.to_string_lossy());
            path.with_file_name(new_name)
        } else {
            // No extension: append timestamp
            let new_name = format!("{}_{}", stem_str, timestamp);
            path.with_file_name(new_name)
        }
    } else {
        // No filename component, just append timestamp (edge case)
        path.with_file_name(timestamp)
    }
}

fn bytes_to_hex(raw: &[u8]) -> Vec<String> {
    raw.iter().map(|b| format!("0x{b:02X}")).collect()
}

fn pdo_value_to_json(v: &PdoRawValue) -> Value {
    match v {
        PdoRawValue::Integer(n) => json!(n),
        PdoRawValue::Unsigned(n) => json!(n),
        PdoRawValue::Float(f) => json!(f),
        PdoRawValue::Text(s) => json!(s),
        PdoRawValue::Bytes(b) => json!(bytes_to_hex(b)),
    }
}

fn format_nmt_command(cmd: &NmtCommand) -> &'static str {
    match cmd {
        NmtCommand::StartRemoteNode => "START",
        NmtCommand::StopRemoteNode => "STOP",
        NmtCommand::EnterPreOperational => "ENTER_PRE_OP",
        NmtCommand::ResetNode => "RESET_NODE",
        NmtCommand::ResetCommunication => "RESET_COMM",
        NmtCommand::Unknown(_) => "UNKNOWN",
    }
}

fn format_nmt_state(state: &NmtState) -> String {
    state.to_string()
}
