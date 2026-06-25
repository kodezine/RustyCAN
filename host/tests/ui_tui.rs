//! TUI integration tests — Phase 2.
//!
//! Tests that require private widget functions or internal command parsers live
//! as inline `#[cfg(test)]` blocks in:
//!   - `host/src/tui/mod.rs`     — 5 parser tests (nmt_parse_*, sdo_*_parse)
//!   - `host/src/tui/widgets.rs` — 5 render tests (TestBackend)
//!
//! This file covers tests that only need the public crate API.

// ─── SDO log ring buffer ──────────────────────────────────────────────────────

/// The SDO log ring buffer must cap at `SDO_LOG_CAP` (50) entries.
/// Verified here because `AppState::push_sdo` and `sdo_log` are `pub`.
#[test]
fn sdo_log_ring_buffer_caps_at_50() {
    use chrono::Utc;
    use rustycan::app::{AppState, SdoLogEntry};
    use rustycan::canopen::sdo::SdoDirection;

    let mut state = AppState::new("test.jsonl".into(), 250_000);
    for i in 0u16..60 {
        state.push_sdo(SdoLogEntry {
            ts: Utc::now(),
            node_id: 1,
            direction: SdoDirection::Read,
            index: i,
            subindex: 0,
            name: format!("entry_{i}"),
            value: None,
            abort_code: None,
        });
    }
    assert_eq!(
        state.sdo_log.len(),
        50,
        "ring buffer must cap at SDO_LOG_CAP=50"
    );
    // Oldest entries (0..9) should have been evicted; newest (10..59) retained.
    assert_eq!(
        state.sdo_log.front().unwrap().index,
        10,
        "oldest 10 entries should have been evicted"
    );
}

// ─── Stubs for tests covered by inline #[cfg(test)] blocks ───────────────────

#[test]
#[ignore = "Phase 2: covered inline in tui/widgets.rs — nmt_panel_shows_node_rows"]
fn nmt_panel_shows_node_rows() {}

#[test]
#[ignore = "Phase 2: covered inline in tui/widgets.rs — nmt_panel_operational_is_green"]
fn nmt_panel_operational_is_green() {}

#[test]
#[ignore = "Phase 2: covered inline in tui/widgets.rs — pdo_panel_shows_signals"]
fn pdo_panel_shows_signals() {}

#[test]
#[ignore = "Phase 2: covered inline in tui/widgets.rs — stats_bar_shows_fps_and_load"]
fn stats_bar_shows_fps_and_load() {}

#[test]
#[ignore = "Phase 2: covered inline in tui/mod.rs — nmt_parse_start"]
fn nmt_parse_start() {}

#[test]
#[ignore = "Phase 2: covered inline in tui/mod.rs — nmt_parse_broadcast_preop"]
fn nmt_parse_broadcast_preop() {}

#[test]
#[ignore = "Phase 2: covered inline in tui/mod.rs — sdo_read_parse"]
fn sdo_read_parse() {}

#[test]
#[ignore = "Phase 2: covered inline in tui/mod.rs — sdo_write_parse"]
fn sdo_write_parse() {}

#[test]
#[ignore = "Phase 2: covered inline in tui/mod.rs — nmt_parse_invalid_no_panic"]
fn nmt_parse_invalid_no_panic() {}
