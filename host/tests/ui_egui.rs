//! egui GUI tests — Phase 1.
//!
//! **Pure logic tests** (no display, no GPU) cover the public API that drives
//! the GUI: node-ID parsing and `AppState` mutations.
//!
//! **Widget snapshot tests** (via `egui_kittest::Harness`) live inside
//! `host/src/gui/mod.rs` as inline `#[cfg(test)]` tests because `ConnectForm`,
//! `MonitorView`, and `bus_load_bar` are private to that module.  Run them with:
//!
//! ```sh
//! cargo test -p rustycan          # runs both inline and integration tests
//! cargo insta review -p rustycan  # accept new snapshot images on first run per OS
//! ```
//!
//! Snapshot files land in `host/src/gui/snapshots/` with an OS-specific suffix.

// ─── Node-ID parser (rustycan::eds::parse_node_id_str) ───────────────────────
//
// The function is already `pub` and has its own inline tests in eds/mod.rs.
// These integration-level tests verify it behaves correctly when called as part
// of the public crate API (i.e. the symbol is reachable and correctly exported).

#[test]
fn node_id_parse_decimal() {
    assert_eq!(rustycan::eds::parse_node_id_str("31"), Some(31));
    assert_eq!(rustycan::eds::parse_node_id_str("1"), Some(1));
    assert_eq!(rustycan::eds::parse_node_id_str("127"), Some(127));
}

#[test]
fn node_id_parse_hex_0x_prefix() {
    assert_eq!(rustycan::eds::parse_node_id_str("0x1F"), Some(31));
    assert_eq!(rustycan::eds::parse_node_id_str("0X1F"), Some(31));
    assert_eq!(rustycan::eds::parse_node_id_str("0x01"), Some(1));
}

#[test]
fn node_id_parse_hex_h_suffix() {
    assert_eq!(rustycan::eds::parse_node_id_str("1FH"), Some(31));
    assert_eq!(rustycan::eds::parse_node_id_str("1fh"), Some(31));
}

#[test]
fn node_id_parse_invalid_returns_none() {
    assert_eq!(rustycan::eds::parse_node_id_str("0"), None); // zero invalid
    assert_eq!(rustycan::eds::parse_node_id_str("128"), None); // above 127
    assert_eq!(rustycan::eds::parse_node_id_str("0x00"), None);
    assert_eq!(rustycan::eds::parse_node_id_str("FFH"), None); // 255 > 127
    assert_eq!(rustycan::eds::parse_node_id_str("abc"), None); // not a number
    assert_eq!(rustycan::eds::parse_node_id_str(""), None); // empty
}

// ─── AppState (rustycan::app::AppState) ──────────────────────────────────────

#[test]
fn app_state_initialises_clean() {
    let state = rustycan::app::AppState::new("test.jsonl".into(), 250_000);
    assert_eq!(state.total_frames, 0);
    assert_eq!(state.fps, 0.0);
    assert_eq!(state.bus_load, 0.0);
    assert_eq!(state.baud_rate, 250_000);
    assert!(state.node_map.is_empty());
}

#[test]
fn app_state_init_nodes_populates_map() {
    let mut state = rustycan::app::AppState::new("test.jsonl".into(), 250_000);
    state.init_nodes(&[(1, "Drive".into()), (2, "IO".into())]);
    assert!(state.node_map.contains_key(&1));
    assert!(state.node_map.contains_key(&2));
    assert_eq!(state.node_map.len(), 2);
}

#[test]
fn app_state_init_nodes_idempotent() {
    let mut state = rustycan::app::AppState::new("test.jsonl".into(), 250_000);
    state.init_nodes(&[(1, "Drive".into())]);
    state.init_nodes(&[(1, "Drive".into())]); // second call must not duplicate
    assert_eq!(state.node_map.len(), 1);
}

// ─── Widget snapshot stubs (Connect / Monitor screens) ───────────────────────
//
// Full-screen snapshots of ConnectForm and MonitorView require textures and
// channels that need an egui CreationContext.  These are left as #[ignore]
// until a harness wrapper is implemented in Phase 1b.
// The bus_load_bar, duplicate-detection, and NMT zone snapshots are already
// covered by inline tests in host/src/gui/mod.rs.

#[test]
#[ignore = "Phase 1b: needs egui texture context for full Connect screen"]
fn snapshot_connect_screen_no_dongle() {
    todo!()
}

#[test]
#[ignore = "Phase 1b: needs egui texture context for full Connect screen"]
fn snapshot_connect_screen_dongle_detected() {
    todo!()
}

#[test]
#[ignore = "Phase 1b: needs egui texture context for full Monitor screen"]
fn snapshot_monitor_nmt_three_nodes() {
    todo!()
}
