use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::canopen::nmt::{NmtCommand, NmtEvent, NmtState};
use crate::canopen::pdo::PdoValue;
use crate::canopen::sdo::{SdoDirection, SdoEvent};

/// Appends newline-delimited JSON log entries to a file.
///
/// Each entry is flushed on write so the log remains readable even if the
/// process is killed.
pub struct EventLogger {
    writer: BufWriter<File>,
}

impl EventLogger {
    pub fn new<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            writer: BufWriter::new(file),
        })
    }

    /// Write a pre-built JSON `Value` as a single log line.
    pub fn log(&mut self, entry: Value) {
        if let Ok(s) = serde_json::to_string(&entry) {
            let _ = writeln!(self.writer, "{s}");
            let _ = self.writer.flush();
        }
    }

    pub fn log_nmt(&mut self, ts: DateTime<Utc>, event: &NmtEvent) {
        let entry = match event {
            NmtEvent::Command { command, target_node } => json!({
                "ts": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "type": "NMT_COMMAND",
                "command": format_nmt_command(command),
                "target_node": target_node,
            }),
            NmtEvent::Heartbeat { node_id, state } => json!({
                "ts": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "type": "NMT_STATE",
                "node": node_id,
                "state": format_nmt_state(state),
            }),
        };
        self.log(entry);
    }

    pub fn log_sdo(&mut self, ts: DateTime<Utc>, event: &SdoEvent, raw: &[u8]) {
        let direction = match event.direction {
            SdoDirection::Read => "READ",
            SdoDirection::Write => "WRITE",
        };
        let mut entry = json!({
            "ts": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "type": format!("SDO_{direction}"),
            "node": event.node_id,
            "index": event.index,
            "subindex": event.subindex,
            "name": event.name,
            "raw": raw,
        });
        if let Some(v) = &event.value {
            entry["value"] = serde_json::to_value(v).unwrap_or(Value::Null);
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
    ) {
        let signals: HashMap<&str, Value> = values
            .iter()
            .map(|v| {
                (
                    v.signal_name.as_str(),
                    serde_json::to_value(&v.value).unwrap_or(Value::Null),
                )
            })
            .collect();

        let entry = json!({
            "ts": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "type": "PDO",
            "node": node_id,
            "pdo_num": pdo_num,
            "signals": signals,
            "raw": raw,
        });
        self.log(entry);
    }
}

impl Drop for EventLogger {
    fn drop(&mut self) {
        let _ = self.writer.flush();
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
