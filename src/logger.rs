use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::canopen::nmt::{NmtCommand, NmtEvent, NmtState};
use crate::canopen::pdo::{PdoRawValue, PdoValue};
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
