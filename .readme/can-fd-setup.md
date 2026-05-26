<!--
  can-fd-setup.md — CAN FD + CANopen FD implementation plan and progress tracker
  Branch: feat/can-fd
  Last updated: 2026-05-26

  HOW TO USE IN A NEW SESSION
  ────────────────────────────
  1. Read the ## Architecture Context section — it replaces reading all source files.
  2. Find the first unchecked [ ] item in ## Progress Tracker. That is where to continue.
  3. After completing a phase, change [ ] to [x], commit, and move to the next.
  4. Update ## Session Notes with any discoveries that affect later phases.
-->

# CAN FD + CANopen FD — Implementation Plan & Tracker

## Quick Reference

| Item | Value |
|---|---|
| **Branch** | `feat/can-fd` |
| **Base branch** | `main` |
| **Primary firmware target** | `dongle-h743` (H743I-EVAL MB1246 Rev E); H753 mirrored in Phase 5 |
| **FD data-phase rates** | 500 kbit/s · 1 Mbit/s · 2 Mbit/s |
| **ISO mode** | Both ISO (ISO 11898-1:2015) and non-ISO (Bosch) selectable |
| **PEAK adapter** | Classic CAN only — `host-can` crate lists FD as upstream TODO |
| **Protocol version** | Stays `0x02` — FD is purely additive, no wire format change |
| **Wire format** | Unchanged 80 bytes; FD/BRS/ESI flag bits already defined in `FrameFlags` |

---

## Architecture Context

> Read this before touching any file. These are verified facts about the current codebase.

### Protocol (`kcan-protocol/`)

- `KCanFrame` (80 bytes): flags byte already has `FD=0x04`, `BRS=0x08`, `ESI=0x10`; data field is 64 bytes — **no wire format change needed**
- `KCAN_VERSION = 0x02` — stays; FD is additive
- `KCanMode._reserved[0]` → rename to `fd_flags: u8` (bit 0 = FD_ENABLED, bit 1 = NON_ISO); backward compat: old firmware ignores reserved bytes
- `SET_FD_BITTIMING` (request code `0x03`) already in `RequestCode` enum — just not called anywhere yet
- `KCanBtConst::H753_64MHZ` has misleading name; actual `clock_hz = 32_000_000` — fix in Phase 1

### Firmware — current state (`firmware/dongle-h743/src/`)

- `main.rs`: FDCAN1 init hardcodes `can1_cfg.set_bitrate(250_000); can1_cfg.into_normal_mode()` — **250 kbps fixed**
- `ep0_handler.rs`: `SET_BITTIMING` is ACKed but **not applied** to FDCAN hardware (only updates LCD display)
- `ep0_handler.rs`: `SET_FD_BITTIMING` match arm absent — firmware returns None (STALL) for code 0x03
- `ep0_handler.rs`: `SET_MODE` correctly sets `LISTEN_ONLY` atomic and signals `BULK_RESTART`
- `can_task.rs`: uses `Frame` (classic CAN) only; `kcan_to_frame()` truncates payload to 8 bytes
- FDCAN kernel clock: **32 MHz** (PLL2Q = 320 MHz / 10)
- **`CanConfigurator` must be configured before `into_normal_mode()`** — cannot reconfigure after bus-on

### Firmware — key statics

```rust
// in usb_task.rs
static LISTEN_ONLY: AtomicBool          // set by ep0_handler on SET_MODE
static BULK_RESTART: Signal<_, ()>      // fires on SET_MODE bus-on / bus-off
static USB_CONFIGURED: Signal<_, bool>  // fires on USB enumeration / disconnect
```

### Host (`host/src/`)

- `adapters/kcan.rs`: `KCanAdapter::open(serial, baud, listen_only)` — FD params absent
- `adapters/peak.rs`: thin wrapper over `host-can`; no FD capability
- `session.rs` → `SessionConfig { baud: u32, listen_only: bool, ... }` — needs `fd_data_baud: Option<u32>`, `iso_mode: bool`
- `canopen/mod.rs`: `FrameType::Emergency(u8)` recognized but **payload not decoded** anywhere
- `canopen/pdo.rs`: `PdoDecoder` implicitly capped at 8-byte payload; scan limited to PDOs 1–4
- `eds/mod.rs`: INI-format EDS parser only; no XDD support

---

## Session Notes

> Update this section with discoveries that affect later phases.

| Date | Phase | Note |
|---|---|---|
| 2026-05-26 | 0 | Embassy FDCAN FD API — see Phase 0 results below |

---

## Progress Tracker

### Physical Layer

- [x] **Phase 0** — Embassy FDCAN FD API discovery *(prerequisite)*

  **Findings** (embassy-stm32 0.6.0 verified from Cargo registry source):
  - `CanConfigurator::set_fd_data_bitrate(bitrate: u32, transceiver_delay_compensation: bool)` ✅ — sets `config.frame_transmit = AllowFdCanAndBRS` and calls `calc_can_timings()` for data phase
  - `CanConfigurator::set_config(FdCanConfig)` ✅ — `FdCanConfig::set_non_iso_mode(bool)` controls CCCR.NISO
  - **FD frame type**: `FdFrame` (not `FdCanFrame`) — `FdEnvelope { ts: Timestamp, frame: FdFrame }`
  - **TX FD**: `CanTx::write_fd(&FdFrame) -> Option<FdFrame>` ✅ — `CanRx::read_fd() -> Result<FdEnvelope, BusError>` ✅
  - **Header**: `Header::new_fd(id, len, rtr, brs)` ✅; `header.fdcan() -> bool`, `header.bit_rate_switching() -> bool`
  - **DataBitTiming** fields: `prescaler: NonZeroU16` (1–31), `seg1: NonZeroU8` (1–31), `seg2: NonZeroU8` (1–15), `sync_jump_width: NonZeroU8` (1–15), `transceiver_delay_compensation: bool`
  - `CanConfigurator<'d>` — lifetime `'d` is the peripheral borrow. When `p.FDCAN1` comes from `embassy_stm32::init()` in `main`, it is `'static`, so `CanConfigurator<'static>` is valid and satisfies embassy task spawn bounds ✅
  - **Buffered FD variant**: `Can::buffered_fd::<TX, RX>()` returns `BufferedCanFd`; splits to `BufferedFdCanSender` / `BufferedFdCanReceiver` — not needed (existing code uses `split()` + manual join)
  - **Classic path unchanged**: `CanRx::read() -> Result<Envelope, BusError>`, `CanTx::write(&Frame) -> Option<Frame>` still present; FD mode FDCAN hardware accepts both classic and FD frames on RX simultaneously

- [x] **Phase 1** — `kcan-protocol` crate
  - Files: [`kcan-protocol/src/control.rs`](../kcan-protocol/src/control.rs)
  - [x] Add `KCanBtConst::H743_32MHZ` and `H753_32MHZ`; deprecated `H753_64MHZ` (misnomer)
  - [x] Add `KCanBitTiming::for_fd_data_bitrate(clock_hz, data_bitrate) -> Option<Self>` — BRP cap 31, 16 TQ (TSEG1=11/TSEG2=4/SJW=4)
  - [x] Added `KCanModeFdFlags { FD_ENABLED=0x01, NON_ISO=0x02 }`
  - [x] Extended `KCanMode`: `_reserved[u8;3]` → `fd_flags: u8, _reserved: [u8;2]`; added `bus_on_fd()`; updated `to_bytes`/`from_bytes`
  - [x] Added `KCanFdConfig { nominal_baud: u32, fd_timing: Option<KCanBitTiming>, iso: bool, mode_flags: u8 }`

- [x] **Phase 2** — Firmware deferred FDCAN init (H743)
  - Files: [`firmware/dongle-h743/src/main.rs`](../firmware/dongle-h743/src/main.rs), [`firmware/dongle-h743/src/can_task.rs`](../firmware/dongle-h743/src/can_task.rs), [`firmware/dongle-h743/src/usb_task.rs`](../firmware/dongle-h743/src/usb_task.rs)
  - [x] Added `static CAN_CONFIG: Signal<CriticalSectionRawMutex, KCanFdConfig>` in `usb_task.rs`
  - [x] `main()`: builds `can1_cfg` (`CanConfigurator`) but does NOT call `into_normal_mode()` — passes to `can_task`; pre-signals 250k default (removed in Phase 3)
  - [x] `can_task`: signature now takes `CanConfigurator<'static>`; awaits `CAN_CONFIG.wait()` → applies baud → starts bus → splits and runs RX/TX loops

- [x] **Phase 3** — EP0 handler: wire timing to FDCAN (H743)
  - Files: [`firmware/dongle-h743/src/ep0_handler.rs`](../firmware/dongle-h743/src/ep0_handler.rs), [`firmware/dongle-h743/src/main.rs`](../firmware/dongle-h743/src/main.rs)
  - [x] Added `PENDING_NOMINAL_BT` and `PENDING_FD_BT` statics (`Mutex<CriticalSectionRawMutex, Cell<Option<KCanBitTiming>>>`)
  - [x] `SET_BITTIMING` handler: stores in `PENDING_NOMINAL_BT` (was: display-only)
  - [x] Added `SET_FD_BITTIMING` (0x03) handler: parse 16-byte payload → store in `PENDING_FD_BT`
  - [x] `SET_MODE` BUS_ON: reads `fd_flags` byte; assembles `KCanFdConfig` → signals `CAN_CONFIG`
  - [x] Removed Phase 2 pre-signal from `main.rs` — firmware now waits for real EP0 host sequence

- [x] **Phase 4** — `can_task.rs` FD frame encode/decode (H743)
  - Files: [`firmware/dongle-h743/src/can_task.rs`](../firmware/dongle-h743/src/can_task.rs)
  - [x] On `CAN_CONFIG.wait()`: if `fd_timing` is `Some` → builds `DataBitTiming`, sets `FrameTransmissionConfig::AllowFdCanAndBRS`, sets `non_iso_mode(!iso)`
  - [x] RX loop: FD mode uses `rx.read_fd()` (handles both classic and FD frames); classic mode uses `rx.read()`
  - [x] Added `fd_frame_to_kcan()`: maps `Header` fields to `FrameFlags::FD/BRS/EFF/RTR`; full 64-byte payload copy
  - [x] TX loop: checks `kf.flags & FD` → calls `tx.write_fd(&FdFrame)` or `tx.write(&Frame)`
  - [x] Added `kcan_to_fd_frame()`: builds `FdFrame` with `Header::new_fd(id, dlc, rtr, brs)`
  - [x] Import fix: `embassy_stm32::can::config` (not `::can::fd::config` — `fd` is private)

- [x] **Phase 5** — dongle-h753 mirror *(parallel with 2–4)*
  - Files: [`firmware/dongle-h753/src/main.rs`](../firmware/dongle-h753/src/main.rs), [`firmware/dongle-h753/src/ep0_handler.rs`](../firmware/dongle-h753/src/ep0_handler.rs), [`firmware/dongle-h753/src/can_task.rs`](../firmware/dongle-h753/src/can_task.rs), [`firmware/dongle-h753/src/usb_task.rs`](../firmware/dongle-h753/src/usb_task.rs)
  - [x] `usb_task.rs`: added `CAN_CONFIG: Channel<_, KCanFdConfig, 2>` (capacity 2, one slot per FDCAN channel)
  - [x] `ep0_handler.rs`: added `PENDING_NOMINAL_BT` + `PENDING_FD_BT` statics; `SET_BITTIMING` stores timing; added `SET_FD_BITTIMING (0x03)` handler; `SET_MODE BUS_ON` assembles `KCanFdConfig` and `try_send`s TWO copies (one per channel)
  - [x] `can_task.rs`: signature changed from `Can<'static>` to `CanConfigurator<'static>`; awaits `CAN_CONFIG.receive()`; applies optional `DataBitTiming`; replaced `select()` loop with `join()` (RX/TX concurrent, no TX starvation); added FD RX/TX frame handling and helpers
  - [x] `main.rs`: removed `set_bitrate()`/`into_normal_mode()` pre-init; passes `can1_cfg`/`can2_cfg` (as `CanConfigurator`) to both `can_task` spawns

- [x] **Phase 6** — Host adapter FD params
  - Files: [`host/src/adapters/kcan.rs`](../host/src/adapters/kcan.rs), [`host/src/adapters/mod.rs`](../host/src/adapters/mod.rs)
  - [x] `KCanAdapter::open()` gains `fd_data_baud: Option<u32>`, `iso_mode: bool` params
  - [x] After `SET_BITTIMING`: if FD, compute `KCanBitTiming::for_fd_data_bitrate()` → send `SET_FD_BITTIMING`; error if bitrate not achievable
  - [x] `SET_MODE` payload: `KCanMode::bus_on_fd()` with `FD_ENABLED` + optionally `NON_ISO` when FD; classic path unchanged
  - [x] `open_adapter()` in `mod.rs` gains same two params and passes them through; PEAK arm ignores them
  - [x] Call sites in `session.rs` (×2) and `bbd/main.rs` updated with `None, true` placeholders (Phase 7 wires real config)

- [x] **Phase 7** — Session + JSON config
  - Files: [`host/src/session.rs`](../host/src/session.rs), [`host/src/gui/mod.rs`](../host/src/gui/mod.rs), [`host/config.example.json`](../host/config.example.json), [`host/config.kcan.json`](../host/config.kcan.json), [`host/config.kcan-h743.json`](../host/config.kcan-h743.json)
  - [x] `SessionConfig` gains `fd_data_baud: Option<u32>` and `iso_mode: bool` (default `true`)
  - [x] `session.rs` propagates both fields to `open_adapter()` calls (replacing `None, true` placeholders)
  - [x] `PersistedConfig` gains `fd_data_baud: Option<u32>` + `#[serde(default)]` and `iso_mode: bool` + `#[serde(default = "default_true")]` — backward-compatible with existing config files
  - [x] `ConnectForm` gains `fd_data_baud: Option<u32>` and `iso_mode: bool` fields (no UI yet — Phase 8)
  - [x] `into_form()`, `From<&ConnectForm>`, `Default`, `try_connect()`, `load_session_config()` all updated
  - [x] JSON config files updated with `"fd_data_baud": null, "iso_mode": true`

- [x] **Phase 8** — GUI FD controls
  - Files: [`host/src/gui/mod.rs`](../host/src/gui/mod.rs)
  - [x] "CAN FD:" row in the Connection settings grid (KCAN-only via `add_enabled_ui`)
  - [x] "Enable FD + BRS" checkbox toggles `fd_data_baud` (`None` ↔ `Some(2_000_000)`)
  - [x] Data-rate `ComboBox`: 500 kbit/s / 1 Mbit/s / 2 Mbit/s (visible only when FD enabled)
  - [x] "ISO 11898-1:2015" checkbox for `iso_mode` with hover tooltip (visible when FD enabled)

- [x] **Phase 9** — Passive auto-baud detection
  - Files: [`host/src/session.rs`](../host/src/session.rs), [`host/src/adapters/kcan.rs`](../host/src/adapters/kcan.rs)
  - [x] `auto_detect_baud_inner()`: iterate `[10k, 20k, 50k, 100k, 125k, 250k, 500k, 800k, 1M]` listen-only, 2 s each; return first rate with valid frames *(committed in phase-9)*
  - [x] `auto_detect_fd_baud_inner()` *(phase-9b)*: scan FD data rates `[2M, 1M, 500k]` descending using BRS flag detection; returns highest with BRS frames seen; wired into `open()` after nominal baud detected

---

### Application Layer (CANopen FD)

- [x] **Phase 10** — XDD parser (CiA 311) *(parallel with Phase 9)*
  - Files: new [`host/src/xdd/mod.rs`](../host/src/xdd/mod.rs), [`host/Cargo.toml`](../host/Cargo.toml)
  - [x] Added `roxmltree = "0.20"` dep to `host/Cargo.toml`
  - [x] `parse_xdd(path) -> Result<ObjectDictionary>`: walks `<CANopenObject>` + `<CANopenSubObject>`; emits same `OdEntry` structs as `parse_eds()`; unit test `parse_minimal_xdd`
  - [x] `session.rs`: `parse_od_file()` helper dispatches by extension — `.xdd`/`.xml` → `parse_xdd`, else → `parse_eds`
  - [x] GUI file picker: filter updated to `"EDS / XDD"` with `.eds`, `.xdd`, `.xml` extensions

- [x] **Phase 11** — FD PDO engine expansion *(depends on Phase 10)*
  - Files: [`host/src/canopen/pdo.rs`](../host/src/canopen/pdo.rs)
  - [x] Added `pub fn fd_dlc_to_bytes(dlc: u8) -> usize`: 0–8 unchanged; 9→12, 10→16, 11→20, 12→24, 13→32, 14→48, 15→64
  - [x] `PdoDecoder::from_od()`: expanded scan to full CiA 301 ranges `0x1800–0x19FF` / `0x1A00–0x1BFF` / `0x1400–0x15FF` / `0x1600–0x17FF` (up to 512 PDOs each); early-termination guard

- [x] **Phase 12** — EMCY decode + CiA 301 error table *(parallel with 10/11)*
  - Files: new [`host/src/canopen/emcy.rs`](../host/src/canopen/emcy.rs), `canopen/mod.rs`
  - [x] `EmcyEvent { node_id: u8, error_code: u16, error_register: u8, vendor_data: Vec<u8> }`
  - [x] `decode_emcy(node_id, data) -> Option<EmcyEvent>`: bytes 0–1 error code LE, byte 2 register, 3..N vendor
  - [x] `describe_error_code(u16) -> &'static str`: ~50-entry CiA 301 table with class-range fallback
  - [x] `describe_error_register(u8) -> String`: formats active bits (Generic/Voltage/Temp/Communication/Device Profile/Manufacturer)
  - [x] Integration (Phase 13): `CanEvent::Emcy`, `app.rs` ring buffer, `session.rs` Emergency arm, `logger.rs` `log_emcy`, GUI `emcy_section`, TUI arm

- [x] **Phase 13** — USDO full TX+RX (CiA 602) *(depends on Phase 12 — last)*
  - Files: new [`host/src/canopen/usdo.rs`](../host/src/canopen/usdo.rs), `canopen/mod.rs`, `app.rs`, `session.rs`, `logger.rs`, `gui/mod.rs`, `tui/mod.rs`, `tui/log_stream.rs`
  - [x] Default COB-IDs: `USDO_COB_CLIENT = 0x7E5`, `USDO_COB_SERVER = 0x7E9`
  - [x] USDO header (8 bytes): node address(2) + CSS/SCS(1) + command(1) + index(2) + subindex(1) + length(1); data up to 56 bytes
  - [x] `decode_usdo(cob_id, data) -> Option<UsdoEvent>`; unit tests (read/write roundtrip, too-short)
  - [x] `encode_usdo_read(node_id, index, subindex) -> Vec<u8>` (64-byte FD payload)
  - [x] `encode_usdo_write(node_id, index, subindex, data) -> Vec<u8>`
  - [x] `classify_frame()`: `FrameType::UsdoRequest` (0x7E5) / `FrameType::UsdoResponse` (0x7E9)
  - [x] `CanEvent::Usdo(UsdoEvent)` in `app.rs`; `usdo_log` ring buffer (cap 50); `push_usdo()`
  - [x] `session.rs`: USDO + EMCY match arms in `recv_loop`; both imports
  - [x] `logger.rs`: `log_emcy()` + `log_usdo()` JSONL methods
  - [x] GUI: `emcy_section()` (collapsing, node/code/description/register) + `usdo_section()` (collapsing, R/W/abort rows)
  - [x] TUI: `Emcy` and `Usdo` arms in both `log_stream.rs` and `mod.rs` event formatters

---

### Cross-cutting (after all phases complete)

- [ ] **Phase 8b** — Documentation
  - New `.readme/can-fd.md`: physical FD guide (rates, ISO/non-ISO, GUI controls, JSONL FD fields, firmware-reset limitation)
  - New `.readme/canopen-fd.md`: application-layer guide (XDD vs EDS, USDO vs SDO, FD PDO, EMCY, node-guarding policy)
  - Update `.readme/features.md`: `CAN FD | planned` → `✅`; add rows for XDD, USDO, FD PDO, EMCY
  - Update `.readme/gui-guide.md`: FD connect-form controls; EMCY panel; USDO protocol selector; XDD in file picker
  - Update `.readme/jsonl-format.md`: `fd`/`brs`/`esi` fields on `rx`/`tx` events; new `emcy` and `usdo` event types
  - Update `README.md`: CAN FD + CANopen FD bullets; links to new docs

- [ ] **Phase 13c** — Doorstop requirements
  - New SOUP026 (CAN FD physical), SOUP027 (FD rates), SOUP028 (XDD), SOUP029 (USDO), SOUP030 (FD PDO), SOUP031 (EMCY), SOUP032 (no node guarding)
  - Amend SOUP019 (baud rate): FD data phase is KCAN-only; `host-can` FD noted as pending
  - Amend SOUP003 (KCAN support): add H743XI; link SOUP026 + SOUP029
  - New SOUPTC027–033 (FD RX/TX, regression, ISO, XDD, USDO, PDO, EMCY)
  - New SOUPANOM007 (firmware reset to change FD config)
  - New SOUPANOM008 (USDO disabled in classic CAN mode)

---

## Verification Checklist

- [ ] Classic CAN regression: H743 at 250 kbps; NMT/PDO unchanged — `cargo test -p rustycan -- can_fd`
- [ ] FD RX: external FD frame at 500k/2M → JSONL shows `fd=true brs=true` + full payload
- [ ] FD TX: SendRaw FD frame → confirmed on bus with external tool
- [ ] ISO vs non-ISO: RTT trace or scope confirms FDCAN CCCR.NISO bit changes correctly
- [ ] H743 smoke: classic + FD mode both operational
- [ ] H753 smoke: classic + FD mode both operational (dual-channel)
- [ ] Auto-baud: live 250k bus → "Auto-detect" → correct rate within 10 s
- [ ] XDD parser round-trip — `cargo test -p rustycan -- xdd`
- [ ] FD PDO 64-byte decode — `cargo test -p rustycan -- pdo::fd`
- [ ] EMCY decode + CiA 301 lookup — `cargo test -p rustycan -- emcy`
- [ ] USDO encode/decode round-trip — `cargo test -p rustycan -- usdo`

---

## Critical Risks

| # | Risk | Phase | Mitigation |
|---|---|---|---|
| 1 | Embassy `set_fd_data_bitrate()` may not exist in 0.6 | 0 | Phase 0 must verify before Phase 2–4; fallback: PAC CCCR register direct write |
| 2 | `CanConfigurator<'static>` may not satisfy embassy task spawn | 0 | Fallback: pass via `Signal<KCanFdConfig>` and rebuild config in task |
| 3 | TJA1044 at 2 Mbit/s — eval board transceiver rated limit unknown | 4 | Confirm part number + datasheet before testing 2M |
| 4 | FDCAN re-init requires replug | 2 | Documented in SOUPANOM007; GUI shows "Replug to change FD settings" |
| 5 | PEAK CAN FD blocked by `host-can` upstream | — | PEAK stays classic CAN; track upstream; separate branch when available |

---

## Dependency Graph

```
Phase 0 (Embassy API)
  └─► Phase 1 (kcan-protocol)
        ├─► Phase 2 (deferred init) ──► Phase 3 (EP0) ──► Phase 4 (can_task)
        │                                                        └─► Phase 5 (H753)
        └─► Phase 6 (host adapter) ──► Phase 7 (session) ──► Phase 8 (GUI)
                                                                    └─► Phase 9 (auto-baud)

Phase 10 (XDD) ──────────────────────────────────────────► Phase 11 (FD PDO)
Phase 12 (EMCY) ─────────────────────────────────────────► Phase 13 (USDO)

Phase 8b (docs) + Phase 13c (doorstop) ◄── all phases complete
```

Phases 10 and 12 are independent of each other and can run in parallel after Phase 1.
