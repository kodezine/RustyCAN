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

- [ ] **Phase 5** — dongle-h753 mirror *(parallel with 2–4)*
  - Files: [`firmware/dongle-h753/src/main.rs`](../firmware/dongle-h753/src/main.rs), [`firmware/dongle-h753/src/ep0_handler.rs`](../firmware/dongle-h753/src/ep0_handler.rs), [`firmware/dongle-h753/src/can_task.rs`](../firmware/dongle-h753/src/can_task.rs)
  - Same changes as Phases 2–4; key difference: dual FDCAN channels (both `can1_cfg` + `can2_cfg` get FD config)

- [ ] **Phase 6** — Host adapter FD params
  - Files: [`host/src/adapters/kcan.rs`](../host/src/adapters/kcan.rs)
  - [ ] `KCanAdapter::open()` gains `fd_data_baud: Option<u32>`, `iso_mode: bool` params
  - [ ] After `SET_BITTIMING`: if FD, compute data timing via `KCanBitTiming::for_fd_data_bitrate()` → send `SET_FD_BITTIMING`
  - [ ] `SET_MODE` payload: populate `fd_flags` byte (`FD_ENABLED` + optionally `NON_ISO`)
  - [ ] PEAK adapter: no changes — new params silently ignored in `open_adapter()` dispatch

- [ ] **Phase 7** — Session + JSON config
  - Files: [`host/src/session.rs`](../host/src/session.rs), [`host/config.example.json`](../host/config.example.json), [`host/config.kcan.json`](../host/config.kcan.json), [`host/config.kcan-h743.json`](../host/config.kcan-h743.json)
  - [ ] `SessionConfig` gains `fd_data_baud: Option<u32>` and `iso_mode: bool` (default `true`)
  - [ ] Persist to / restore from JSON (keys `"fd_data_baud"`, `"iso_mode"`)

- [ ] **Phase 8** — GUI FD controls
  - Files: [`host/src/gui/mod.rs`](../host/src/gui/mod.rs)
  - [ ] Connect form (KCAN only): "CAN FD" checkbox; "Data rate" dropdown (500k / 1M / 2M); "ISO CAN FD" checkbox
  - [ ] Dropdown + ISO checkbox hidden when `!fd_enabled` or PEAK adapter selected
  - [ ] Persist `fd_enabled`, `fd_data_baud`, `iso_mode` in config JSON

- [ ] **Phase 9** — Passive auto-baud detection
  - Files: [`host/src/session.rs`](../host/src/session.rs), [`host/src/gui/mod.rs`](../host/src/gui/mod.rs)
  - [ ] "Auto-detect" option in nominal baud dropdown (KCAN only)
  - [ ] `auto_detect_baud()`: iterate `[10k, 20k, 50k, 100k, 125k, 250k, 500k, 800k, 1M]` listen-only, 2 s each; return first rate with valid frames
  - [ ] After nominal found: scan FD data rates `[500k, 1M, 2M]` — return highest with BRS frames seen

---

### Application Layer (CANopen FD)

- [ ] **Phase 10** — XDD parser (CiA 311) *(parallel with Phase 9)*
  - Files: new [`host/src/xdd/mod.rs`](../host/src/xdd/mod.rs), [`host/Cargo.toml`](../host/Cargo.toml)
  - [ ] Add `roxmltree = "0.20"` dep to `host/Cargo.toml`
  - [ ] `parse_xdd(path) -> Result<ObjectDictionary>`: walk `<CANopenObject>` (attrs: `index`, `name`, `objectType`, `dataType`, `defaultValue`, `PDOmapping`) + `<CANopenSubObject>` (attrs: `subIndex`, `name`, `dataType`, `defaultValue`, `accessType`) → emit same `OdEntry` structs as `parse_eds()`
  - [ ] Extract `nrOfEntries` sub-object value for extended PDO count (> 4 PDOs)
  - [ ] `session.rs`: auto-detect by extension — `.eds` → `parse_eds`, `.xdd`/`.xml` → `parse_xdd`
  - [ ] GUI file picker: add `.xdd`, `.xml` to filter

- [ ] **Phase 11** — FD PDO engine expansion *(depends on Phase 10)*
  - Files: [`host/src/canopen/pdo.rs`](../host/src/canopen/pdo.rs)
  - [ ] Add `fd_dlc_to_bytes(dlc: u8) -> usize`: 0–8 unchanged; 9→12, 10→16, 11→20, 12→24, 13→32, 14→48, 15→64
  - [ ] `PdoDecoder::decode()`: replace 8-byte cap with `fd_dlc_to_bytes(frame.dlc())`
  - [ ] `PdoDecoder::from_od()`: expand TPDO/RPDO scan to full range `0x1800–0x19FF` / `0x1A00–0x1BFF` / `0x1400–0x15FF` / `0x1600–0x17FF`; stop when no `nrOfEntries` sub-object

- [ ] **Phase 12** — EMCY decode + CiA 301 error table *(parallel with 10/11)*
  - Files: new [`host/src/canopen/emcy.rs`](../host/src/canopen/emcy.rs), + `mod.rs`, `app.rs`, `session.rs`, `gui/mod.rs`, `logger.rs`
  - [ ] `EmcyEvent { node_id: u8, error_code: u16, error_register: u8, vendor_data: Vec<u8> }`
  - [ ] `decode_emcy(node_id, frame) -> Option<EmcyEvent>`: bytes 0–1 error code LE, byte 2 register, 3..N vendor (up to 5 classic / 61 FD)
  - [ ] `describe_error_code(u16) -> &'static str`: ~50-entry CiA 301 table — 0x0000 No error; 0x1xxx Generic; 0x2xxx Current; 0x3xxx Voltage; 0x4xxx Temp; 0x5xxx HW; 0x6xxx SW; 0x7xxx Modules; 0x8xxx Monitoring; 0x9xxx External; 0xFxxx Additional
  - [ ] Error register bit descriptions: bits 0/2/3/4/5/7 = Generic/Voltage/Temp/Comm/Profile/Manufacturer
  - [ ] `CanEvent::Emcy(EmcyEvent)` in `app.rs`; per-node ring buffer (last 20 events)
  - [ ] JSONL: `{"type":"emcy","node_id":N,"error_code":"0xXXXX","description":"...","error_register":"0xXX","vendor_data":"HEX"}`
  - [ ] GUI: collapsing "EMCY" sub-panel per node; badge count; error register bit breakdown
  - [ ] Tooltip: "Node guarding not implemented — heartbeat protocol only"

- [ ] **Phase 13** — USDO full TX+RX (CiA 602) *(depends on Phase 12 — last)*
  - Files: new [`host/src/canopen/usdo.rs`](../host/src/canopen/usdo.rs), + `mod.rs`, `app.rs`, `session.rs`, `gui/mod.rs`
  - [ ] Default COB-IDs: client→server `0x7E5`, server→client `0x7E9`
  - [ ] USDO header (8 bytes): node address(2) + SCS/CSS(1) + command(1) + index(2) + subindex(1) + length(1); remainder = data (up to 56 bytes)
  - [ ] `decode_usdo(cob_id, data) -> Option<UsdoEvent>`
  - [ ] `encode_usdo_read(node_id, index, subindex) -> Vec<u8>` (64-byte FD payload)
  - [ ] `encode_usdo_write(node_id, index, subindex, data) -> Vec<u8>`
  - [ ] `classify_frame()`: add `FrameType::UsdoRequest` (0x7E5) / `FrameType::UsdoResponse` (0x7E9)
  - [ ] `CanCommand::UsdoRead` / `UsdoWrite` variants; session correlates request/response pairs
  - [ ] `CanEvent::Usdo(UsdoEvent)` in `app.rs`; USDO log ring buffer alongside SDO log
  - [ ] GUI: per-node "Protocol" selector (Classic SDO / USDO); USDO disabled in classic CAN mode
  - [ ] JSONL: `{"type":"usdo", ...}` events mirroring SDO format

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
