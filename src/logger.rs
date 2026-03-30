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
    /// Count of log entries since last flush.
    entries_since_flush: usize,
    /// Time of last flush.
    last_flush: Instant,
    /// Flush every N entries.
    flush_interval_entries: usize,
    /// Flush at least every N milliseconds.
    flush_interval_ms: u64,
}

impl EventLogger {
    pub fn new<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        Self::with_config(path, 50, 100)
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
            entries_since_flush: 0,
            last_flush: Instant::now(),
            flush_interval_entries,
            flush_interval_ms,
        })
    }

    /// Write a pre-built JSON `Value` as a single log line.
    /// Flushes periodically to balance data safety with performance.
    pub fn log(&mut self, entry: Value) {
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
        self.entries_since_flush = 0;
        self.last_flush = Instant::now();
    }

    /// Log an NMT master command that was sent by this application.
    pub fn log_nmt_sent(
        &mut self,
        ts: DateTime<Utc>,
        command: &NmtCommand,
        target_node: u8,
        raw: &[u8],
    ) {
        let entry = json!({
            "ts": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "type": "NMT_COMMAND_SENT",
            "cob_id": "0x000",
            "command": format_nmt_command(command),
            "target_node": target_node,
            "raw": bytes_to_hex(raw),
        });
        self.log(entry);
    }

    pub fn log_nmt(&mut self, ts: DateTime<Utc>, event: &NmtEvent, raw: &[u8], cob_id: u16) {
        let entry = match event {
            NmtEvent::Command {
                command,
                target_node,
            } => json!({
                "ts": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "type": "NMT_COMMAND",
                "cob_id": format!("0x{cob_id:03X}"),
                "command": format_nmt_command(command),
                "target_node": target_node,
                "raw": bytes_to_hex(raw),
            }),
            NmtEvent::Heartbeat { node_id, state } => json!({
                "ts": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "type": "NMT_STATE",
                "cob_id": format!("0x{cob_id:03X}"),
                "node": node_id,
                "state": format_nmt_state(state),
                "raw": bytes_to_hex(raw),
            }),
        };
        self.log(entry);
    }

    pub fn log_sdo(&mut self, ts: DateTime<Utc>, event: &SdoEvent, raw: &[u8], cob_id: u16) {
        let direction = match event.direction {
            SdoDirection::Read => "READ",
            SdoDirection::Write => "WRITE",
        };
        let mut entry = json!({
            "ts": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "type": format!("SDO_{direction}"),
            "cob_id": format!("0x{cob_id:03X}"),
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
        let mut signals = serde_json::Map::new();
        for v in values {
            signals.insert(v.signal_name.clone(), pdo_value_to_json(&v.value));
        }

        let entry = json!({
            "ts": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "type": "PDO",
            "cob_id": format!("0x{cob_id:03X}"),
            "node": node_id,
            "pdo_num": pdo_num,
            "signals": signals,
            "raw": bytes_to_hex(raw),
        });
        self.log(entry);
    }
}

impl Drop for EventLogger {
    fn drop(&mut self) {
        let _ = self.writer.flush();
    }
}

// ─── Private helpers ───────────────────────────────────────────────────────────

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
