//! Headless CAN-sniffer model — no UI dependencies.
//!
//! This crate holds all the *logic* for the RustyCAN sniffer so it can be
//! unit-tested without a GUI and reused by both the dev harness and the live
//! RustyCAN app:
//!
//! * [`SniffFrame`] — one observed (or transmitted) CAN frame.
//! * [`SnifferModel`] — aggregate-by-ID table, filtering, selection, decoded
//!   value inspector state, and periodic-TX scheduling.
//! * [`Replay`] — drives a recorded timeline of frames into the model in real
//!   time (play / pause / scrub), used by the JSONL dev harness.
//! * [`SnifferBackend`] — sink for transmitted frames (real adapter in the app,
//!   no-op / loopback in tests and the harness).
//! * [`jsonl`] — hand-rolled parser for RustyCAN's JSONL log format.

use std::collections::BTreeMap;

/// How long (seconds) a changed byte stays highlighted before fully fading.
pub const HIGHLIGHT_SECS: f64 = 1.5;

/// Maximum payload bytes tracked per row (classic CAN).
pub const MAX_BYTES: usize = 8;

// ---------------------------------------------------------------------------
// Frame + row
// ---------------------------------------------------------------------------

/// A single CAN frame, from the bus, a log, or a transmit.
#[derive(Clone, Debug, PartialEq)]
pub struct SniffFrame {
    /// Monotonic-ish timestamp in seconds (live: hardware ns → secs).
    pub ts: f64,
    /// Human display timestamp, e.g. `"13:21:02.416"`.
    pub ts_disp: String,
    /// CAN identifier (COB-ID).
    pub id: u32,
    /// Decoder / kind label (e.g. `PDO`, `NMT_STATE`, `SDO_READ`, `TX`).
    pub typ: String,
    /// Payload bytes (0..=8 for classic CAN).
    pub bytes: Vec<u8>,
    /// `true` if this frame was transmitted by us (TX echo / injected).
    pub is_tx: bool,
}

impl SniffFrame {
    pub fn new(
        ts: f64,
        ts_disp: impl Into<String>,
        id: u32,
        typ: impl Into<String>,
        bytes: Vec<u8>,
    ) -> Self {
        Self {
            ts,
            ts_disp: ts_disp.into(),
            id,
            typ: typ.into(),
            bytes,
            is_tx: false,
        }
    }
}

/// One aggregated row in the sniffer table (latest state per CAN ID).
#[derive(Clone, Debug)]
pub struct Row {
    pub typ: String,
    pub bytes: Vec<u8>,
    pub count: u64,
    pub last_disp: String,
    /// Time (model clock, secs) each byte last changed. `NEG_INFINITY` = never.
    pub changed: [f64; MAX_BYTES],
    pub is_tx: bool,
}

impl Default for Row {
    fn default() -> Self {
        Self {
            typ: String::new(),
            bytes: Vec::new(),
            count: 0,
            last_disp: String::new(),
            changed: [f64::NEG_INFINITY; MAX_BYTES],
            is_tx: false,
        }
    }
}

/// A CAN frame transmitted repeatedly on a fixed interval.
#[derive(Clone, Debug)]
pub struct PeriodicMsg {
    pub id: u32,
    pub bytes: Vec<u8>,
    pub period_ms: f64,
    pub enabled: bool,
    pub last_sent: f64,
    pub count: u64,
}

impl PeriodicMsg {
    pub fn hex(&self) -> String {
        hex_join(&self.bytes)
    }
}

/// Sink for transmitted frames. The live app forwards to the CAN adapter; the
/// dev harness / tests use [`NullBackend`] (local echo only).
pub trait SnifferBackend {
    fn transmit(&mut self, id: u32, data: &[u8]);
}

/// A backend that drops transmits (frames still echo into the local table).
#[derive(Default)]
pub struct NullBackend;
impl SnifferBackend for NullBackend {
    fn transmit(&mut self, _id: u32, _data: &[u8]) {}
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// Aggregated sniffer state + interaction logic (UI-independent).
#[derive(Default)]
pub struct SnifferModel {
    pub rows: BTreeMap<u32, Row>,

    // Filtering.
    pub filter: String,
    pub hide_heartbeat: bool,

    // Decoded-value inspector.
    pub selected_id: Option<u32>,
    pub selected_byte: usize,
    pub decode_big_endian: bool,
    pub word_multiplier: f64,

    // Transmit.
    pub tx_count: u64,
    pub periodics: Vec<PeriodicMsg>,

    pub status: Option<String>,
}

impl SnifferModel {
    pub fn new() -> Self {
        Self {
            decode_big_endian: true,
            word_multiplier: 0.125,
            ..Default::default()
        }
    }

    /// Feed one observed frame into the aggregation (with change highlight).
    pub fn ingest(&mut self, f: &SniffFrame, now: f64) {
        self.apply(f.id, &f.bytes, &f.typ, &f.ts_disp, Some(now), f.is_tx);
    }

    /// Feed a frame without highlighting changes (used when fast-forwarding).
    pub fn ingest_silent(&mut self, f: &SniffFrame) {
        self.apply(f.id, &f.bytes, &f.typ, &f.ts_disp, None, f.is_tx);
    }

    fn apply(
        &mut self,
        id: u32,
        bytes: &[u8],
        typ: &str,
        disp: &str,
        highlight_now: Option<f64>,
        is_tx: bool,
    ) {
        let row = self.rows.entry(id).or_default();
        if let Some(now) = highlight_now {
            let n = bytes.len().min(MAX_BYTES);
            for (i, &nb) in bytes.iter().take(n).enumerate() {
                // Compare as Option so a first-seen byte (no prior value) is
                // always highlighted, including a new value of 0x00.
                if row.bytes.get(i).copied() != Some(nb) {
                    row.changed[i] = now;
                }
            }
        }
        row.bytes = bytes.to_vec();
        row.typ = typ.to_string();
        row.count += 1;
        row.last_disp = disp.to_string();
        row.is_tx = is_tx;
    }

    /// Manually transmit a frame: forward to the backend and echo locally.
    pub fn send_manual(
        &mut self,
        id: u32,
        bytes: &[u8],
        now: f64,
        backend: &mut dyn SnifferBackend,
    ) {
        backend.transmit(id, bytes);
        self.apply(id, bytes, "TX", "sent", Some(now), true);
        self.tx_count += 1;
        self.status = Some(format!(
            "TX #{} {} [{}]",
            self.tx_count,
            fmt_can_id(id),
            hex_join(bytes)
        ));
    }

    /// Record a frame transmitted by an external periodic-TX thread: echo it into
    /// the table and bump the matching periodic's counter. Used when periodic
    /// scheduling is owned by a dedicated thread rather than `pump_periodics`.
    pub fn note_tx(&mut self, id: u32, bytes: &[u8], now: f64) {
        self.apply(id, bytes, "TX", "periodic", Some(now), true);
        if let Some(m) = self.periodics.iter_mut().find(|m| m.id == id) {
            m.count += 1;
        }
    }

    /// Add a periodic message (fires on next `pump_periodics`).
    pub fn add_periodic(&mut self, id: u32, bytes: Vec<u8>, period_ms: f64) {
        self.periodics.push(PeriodicMsg {
            id,
            bytes,
            period_ms: period_ms.max(1.0),
            enabled: true,
            last_sent: 0.0,
            count: 0,
        });
        self.status = Some(format!("Added periodic {}", fmt_can_id(id)));
    }

    pub fn remove_periodic(&mut self, index: usize) {
        if index < self.periodics.len() {
            self.periodics.remove(index);
        }
    }

    /// Fire any due periodic messages. Returns `true` if any are enabled.
    pub fn pump_periodics(&mut self, now: f64, backend: &mut dyn SnifferBackend) -> bool {
        let mut any = false;
        // Collect due sends first to avoid overlapping borrows.
        let mut due: Vec<(usize, u32, Vec<u8>)> = Vec::new();
        for (i, m) in self.periodics.iter().enumerate() {
            if !m.enabled {
                continue;
            }
            any = true;
            let interval = m.period_ms.max(1.0) / 1000.0;
            if now - m.last_sent >= interval {
                due.push((i, m.id, m.bytes.clone()));
            }
        }
        for (i, id, bytes) in due {
            backend.transmit(id, &bytes);
            self.apply(id, &bytes, "TX", "periodic", Some(now), true);
            let m = &mut self.periodics[i];
            m.last_sent = now;
            m.count += 1;
        }
        any
    }

    /// Whether a row for `id` passes the current filter + heartbeat toggle.
    pub fn row_visible(&self, id: u32) -> bool {
        if self.hide_heartbeat && (0x700..=0x77F).contains(&id) {
            return false;
        }
        let f = self.filter.trim();
        if f.is_empty() {
            return true;
        }
        format!("{id:03x}").contains(&f.to_lowercase())
    }

    /// Any highlight still fading? (drives repaint requests in the UI).
    pub fn animating(&self, now: f64) -> bool {
        self.rows
            .values()
            .flat_map(|r| r.changed.iter())
            .any(|&t| now - t < HIGHLIGHT_SECS)
    }

    /// Decoded values for the currently selected (id, byte), if any.
    pub fn decoded(&self) -> Option<Decoded> {
        let id = self.selected_id?;
        let row = self.rows.get(&id)?;
        if row.bytes.is_empty() {
            return None;
        }
        let bi = self.selected_byte.min(row.bytes.len() - 1);
        let byte = row.bytes[bi];
        let next = row.bytes.get(bi + 1).copied().unwrap_or(0);
        let word = if self.decode_big_endian {
            ((byte as u16) << 8) | next as u16
        } else {
            ((next as u16) << 8) | byte as u16
        };
        Some(Decoded {
            id,
            byte_index: bi,
            byte,
            word,
            result: word as f64 * self.word_multiplier,
        })
    }
}

/// Decoded view of the selected byte/word.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decoded {
    pub id: u32,
    pub byte_index: usize,
    pub byte: u8,
    pub word: u16,
    pub result: f64,
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

/// Drives a recorded timeline of frames into a [`SnifferModel`] in real time.
#[derive(Default)]
pub struct Replay {
    pub frames: Vec<SniffFrame>,
    pub pos: usize,
    pub playing: bool,
    pub speed: f64,
    started_at: f64,
    base_ts: f64,
    pub stream_ts: f64,
}

impl Replay {
    pub fn new(frames: Vec<SniffFrame>) -> Self {
        let base = frames.first().map(|f| f.ts).unwrap_or(0.0);
        Self {
            frames,
            pos: 0,
            playing: false,
            speed: 5.0,
            started_at: 0.0,
            base_ts: base,
            stream_ts: base,
        }
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Clear the model and rewind to the start.
    pub fn reset(&mut self, model: &mut SnifferModel, now: f64) {
        model.rows.clear();
        self.pos = 0;
        self.playing = false;
        self.base_ts = self.frames.first().map(|f| f.ts).unwrap_or(0.0);
        self.stream_ts = self.base_ts;
        self.started_at = now;
    }

    /// Jump to `target` frame index, rebuilding the table without highlights.
    pub fn rebuild_to(&mut self, model: &mut SnifferModel, target: usize, now: f64) {
        model.rows.clear();
        let target = target.min(self.frames.len());
        for f in &self.frames[..target] {
            model.apply(f.id, &f.bytes, &f.typ, &f.ts_disp, None, f.is_tx);
        }
        self.pos = target;
        self.stream_ts = self
            .frames
            .get(target.saturating_sub(1))
            .map(|f| f.ts)
            .unwrap_or(self.base_ts);
        self.base_ts = self.stream_ts;
        self.started_at = now;
    }

    pub fn set_playing(&mut self, play: bool, model: &mut SnifferModel, now: f64) {
        if play && self.pos >= self.frames.len() {
            self.reset(model, now);
        }
        self.playing = play;
        self.base_ts = self.stream_ts;
        self.started_at = now;
    }

    /// Advance playback, ingesting any frames now due. Returns `true` if playing.
    pub fn advance(&mut self, model: &mut SnifferModel, now: f64) -> bool {
        if !self.playing {
            return false;
        }
        self.stream_ts = self.base_ts + (now - self.started_at) * self.speed;
        while self.pos < self.frames.len() && self.frames[self.pos].ts <= self.stream_ts {
            let f = self.frames[self.pos].clone();
            model.ingest(&f, now);
            self.pos += 1;
        }
        if self.pos >= self.frames.len() {
            self.playing = false;
            model.status = Some("Replay finished".to_string());
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `"40 00 10"` style hex string.
pub fn hex_join(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Compact code for a record type, for narrow table columns.
pub fn short_type(typ: &str) -> &str {
    match typ {
        "NMT_STATE" => "NMT",
        "NMT_COMMAND" | "NMT_COMMAND_SENT" => "NMT\u{2192}",
        "PDO" => "PDO",
        "SDO_READ" => "SDO",
        "SDO_WRITE" => "SDOw",
        "RAW_FRAME" => "RAW",
        "DBC_SIGNAL" => "DBC",
        "ADAPTER_DISCONNECTED" => "DISC",
        "TX" => "TX",
        other => other,
    }
}

/// Format a CAN identifier for display: 3-digit hex for standard (11-bit)
/// IDs and 8-digit hex for extended (29-bit) IDs, so extended frames aren't
/// shown with a misleading COB-ID-style width.
pub fn fmt_can_id(id: u32) -> String {
    if id <= 0x7FF {
        format!("0x{id:03X}")
    } else {
        format!("0x{id:08X}")
    }
}

/// Parse a hex CAN id like `"0x600"` / `"600"`.
pub fn parse_hex_u32(s: &str) -> Option<u32> {
    u32::from_str_radix(
        s.trim().trim_start_matches("0x").trim_start_matches("0X"),
        16,
    )
    .ok()
}

/// Parse up to 8 leading non-empty hex byte fields (stops at first empty).
pub fn parse_byte_fields(fields: &[String]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for f in fields {
        let t = f.trim();
        if t.is_empty() {
            break;
        }
        let b = u8::from_str_radix(t.trim_start_matches("0x").trim_start_matches("0X"), 16)
            .map_err(|_| format!("invalid byte '{t}'"))?;
        out.push(b);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// JSONL parsing (hand-rolled — no serde dependency)
// ---------------------------------------------------------------------------

pub mod jsonl {
    use super::SniffFrame;

    /// Result of loading a JSONL log.
    #[derive(Default)]
    pub struct Loaded {
        pub frames: Vec<SniffFrame>,
        pub meta: String,
    }

    fn find_str<'a>(line: &'a str, key: &str) -> Option<&'a str> {
        let pat = format!("\"{key}\":\"");
        let start = line.find(&pat)? + pat.len();
        let rest = &line[start..];
        let end = rest.find('"')?;
        Some(&rest[..end])
    }

    fn extract_raw(line: &str) -> Vec<u8> {
        let Some(i) = line.find("\"raw\":[") else {
            return Vec::new();
        };
        let rest = &line[i + 7..];
        let Some(end) = rest.find(']') else {
            return Vec::new();
        };
        rest[..end]
            .split(',')
            .filter_map(|tok| {
                let t = tok.trim().trim_matches('"');
                let t = t.trim_start_matches("0x").trim_start_matches("0X");
                if t.is_empty() {
                    None
                } else {
                    u8::from_str_radix(t, 16).ok()
                }
            })
            .collect()
    }

    /// Parse `"2026-06-24T13:21:02.416Z"` → (monotonic secs, `"13:21:02.416"`).
    pub fn parse_ts(s: &str) -> (f64, String) {
        let Some((date, time)) = s.split_once('T') else {
            return (0.0, s.to_string());
        };
        let time = time.trim_end_matches('Z');
        let mut dp = date.split('-');
        let _y = dp.next();
        let mo = dp.next().and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
        let day = dp.next().and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
        let mut tp = time.split(':');
        let hh = tp.next().and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
        let mm = tp.next().and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
        let ss = tp.next().and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
        let secs = ((mo * 31.0 + day) * 24.0 + hh) * 3600.0 + mm * 60.0 + ss;
        (secs, time.to_string())
    }

    /// Parse one JSONL line into a [`SniffFrame`] (None for non-frame records).
    pub fn parse_line(line: &str) -> Option<SniffFrame> {
        let typ = find_str(line, "type").unwrap_or("");
        if typ.is_empty() || typ == "session_start" {
            return None;
        }
        let cob = find_str(line, "cob_id")?;
        let id = super::parse_hex_u32(cob)?;
        let (ts, ts_disp) = parse_ts(find_str(line, "ts").unwrap_or(""));
        Some(SniffFrame {
            ts,
            ts_disp,
            id,
            typ: typ.to_string(),
            bytes: extract_raw(line),
            is_tx: typ == "TX" || typ == "TX_ECHO",
        })
    }

    /// Parse an entire JSONL document, capping at `max_frames`.
    pub fn parse_document(text: &str, max_frames: usize) -> Loaded {
        let mut out = Loaded::default();
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            if out.meta.is_empty() && line.contains("\"session_start\"") {
                let adapter = find_str(line, "adapter").unwrap_or("?");
                let baud = line
                    .find("\"baud\":")
                    .map(|i| {
                        line[i + 7..]
                            .split(|c: char| !c.is_ascii_digit())
                            .find(|s| !s.is_empty())
                            .unwrap_or("?")
                    })
                    .unwrap_or("?");
                out.meta = format!("{adapter} @ {baud} baud");
                continue;
            }
            if let Some(f) = parse_line(line) {
                out.frames.push(f);
                if out.frames.len() >= max_frames {
                    break;
                }
            }
        }
        out.frames
            .sort_by(|a, b| a.ts.partial_cmp(&b.ts).unwrap_or(std::cmp::Ordering::Equal));
        out
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(id: u32, bytes: &[u8]) -> SniffFrame {
        SniffFrame::new(0.0, "00:00:00.000", id, "PDO", bytes.to_vec())
    }

    #[test]
    fn aggregates_by_id_and_counts() {
        let mut m = SnifferModel::new();
        m.ingest(&frame(0x201, &[1, 2, 3]), 0.0);
        m.ingest(&frame(0x201, &[1, 2, 4]), 0.0);
        m.ingest(&frame(0x181, &[9]), 0.0);
        assert_eq!(m.rows.len(), 2);
        assert_eq!(m.rows[&0x201].count, 2);
        assert_eq!(m.rows[&0x201].bytes, vec![1, 2, 4]);
    }

    #[test]
    fn highlights_only_changed_bytes() {
        let mut m = SnifferModel::new();
        m.ingest(&frame(0x201, &[1, 2, 3]), 10.0); // first appearance: all flash @10
        m.ingest(&frame(0x201, &[1, 9, 3]), 20.0); // only byte 1 changes @20
        let r = &m.rows[&0x201];
        assert_eq!(r.changed[0], 10.0); // unchanged since t=10
        assert_eq!(r.changed[1], 20.0); // changed at t=20
        assert_eq!(r.changed[2], 10.0); // unchanged since t=10
    }

    #[test]
    fn filter_and_heartbeat_visibility() {
        let mut m = SnifferModel::new();
        assert!(m.row_visible(0x201));
        m.filter = "18".into();
        assert!(m.row_visible(0x181));
        assert!(!m.row_visible(0x201));
        m.filter.clear();
        m.hide_heartbeat = true;
        assert!(!m.row_visible(0x700));
        assert!(m.row_visible(0x201));
    }

    #[test]
    fn decode_word_endianness_and_result() {
        let mut m = SnifferModel::new();
        m.ingest(&frame(0x201, &[0x12, 0x34]), 0.0);
        m.selected_id = Some(0x201);
        m.selected_byte = 0;
        m.word_multiplier = 0.125;
        let d = m.decoded().unwrap();
        assert_eq!(d.byte, 0x12);
        assert_eq!(d.word, 0x1234); // big-endian
        assert!((d.result - (0x1234 as f64 * 0.125)).abs() < 1e-9);
        m.decode_big_endian = false;
        assert_eq!(m.decoded().unwrap().word, 0x3412);
    }

    #[test]
    fn periodic_fires_on_interval() {
        let mut m = SnifferModel::new();
        let mut backend = NullBackend;
        m.add_periodic(0x600, vec![0x40, 0x00], 100.0); // 100 ms
        m.pump_periodics(0.0, &mut backend);
        assert_eq!(m.periodics[0].count, 0); // 0 - 0 < 0.1s, not yet
        m.pump_periodics(0.05, &mut backend);
        assert_eq!(m.periodics[0].count, 0); // still too soon
        m.pump_periodics(0.20, &mut backend);
        assert_eq!(m.periodics[0].count, 1); // first fire
        m.pump_periodics(0.25, &mut backend);
        assert_eq!(m.periodics[0].count, 1); // 0.05 since last, too soon
        m.pump_periodics(0.35, &mut backend);
        assert_eq!(m.periodics[0].count, 2); // 0.15 since last, fires
    }

    #[test]
    fn parses_jsonl_line() {
        let line = r#"{"ts":"2026-06-24T13:21:02.416Z","type":"PDO","cob_id":"0x201","raw":["0x2B","0x00"]}"#;
        let f = jsonl::parse_line(line).unwrap();
        assert_eq!(f.id, 0x201);
        assert_eq!(f.typ, "PDO");
        assert_eq!(f.bytes, vec![0x2B, 0x00]);
        assert_eq!(f.ts_disp, "13:21:02.416");
    }

    #[test]
    fn skips_session_start() {
        let line = r#"{"ts":"2026-06-24T13:21:02.389Z","type":"session_start","adapter":"PEAK","baud":250000}"#;
        assert!(jsonl::parse_line(line).is_none());
    }

    #[test]
    fn replay_advances_by_time() {
        let frames = vec![
            SniffFrame::new(0.0, "t0", 0x201, "PDO", vec![1]),
            SniffFrame::new(1.0, "t1", 0x201, "PDO", vec![2]),
            SniffFrame::new(2.0, "t2", 0x202, "PDO", vec![3]),
        ];
        let mut m = SnifferModel::new();
        let mut r = Replay::new(frames);
        r.speed = 1.0;
        r.set_playing(true, &mut m, 100.0); // wall-clock now = 100
        r.advance(&mut m, 100.0); // stream_ts = 0 → ingest frame @0
        assert_eq!(r.pos, 1);
        r.advance(&mut m, 102.0); // stream_ts = 2 → ingest @1 and @2
        assert_eq!(r.pos, 3);
        assert_eq!(m.rows.len(), 2);
    }
}
