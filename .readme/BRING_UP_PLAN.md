# KCAN Dongle Bring-Up Plan

Covers both supported hardware targets. Each target follows the same seven
phases; progress is tracked independently.

## Status Summary

| Target | Board | Phase 0 | Phase 1 | Phase 2 | Phase 3 | Phase 4 | Phase 5 | Phase 6 |
|--------|-------|---------|---------|---------|---------|---------|---------|---------|
| **dongle-h753** | NUCLEO-H753ZI | ✅ | ✅ | 🔄 ext. bus | 🔄 OUT | ✅ | ✅ partial | ⏳ |
| **dongle-h743** | STM32H743I-EVAL MB1246 Rev E | ✅ | ⏳ | ✅ | ⏳ | ⏳ | ⏳ | ⏳ |

## Target Comparison

| Parameter | dongle-h753 (Nucleo) | dongle-h743 (H743I-EVAL) |
|-----------|---------------------|--------------------------|
| Embassy chip | `stm32h753zi` | `stm32h743xi` |
| probe-rs chip | `STM32H753ZITx` | `STM32H743XIHx` |
| HSE crystal | 8 MHz | 25 MHz |
| PLL1 (480 MHz) | prediv=2, mul=240, divp=2 | prediv=5, mul=192, divp=2 |
| PLL2 (32 MHz FDCAN) | prediv=1, mul=40, divq=10 | prediv=5, mul=64, divq=10 |
| FDCAN1 RX | PD0 | PH14 |
| FDCAN1 TX | PD1 | PH13 |
| FDCAN2 | PB5/PB6 (external module) | not used (single channel) |
| USB OTG FS connector | CN13 Micro-B | CN18 Micro-AB |
| USB OTG FS pins | PA11/PA12 | PA11/PA12 |
| Debug connector | CN1 (ST-LINK on-board) | C23 Micro-USB (ST-LINK V3E) |
| CAN transceiver | External TJA1051T | On-board TJA1044 → CN3 DB9 |
| Heartbeat LED | PB0 (green) | PF10 (green LD1) |
| USB status LED | PE1 (blue) | PA4 (orange LD3) |
| bus-test feature | ✅ (dual-channel cross-test) | ✗ (single channel) |
| Power | USB bus or CN8 barrel jack | Barrel jack recommended; CN18 VBUS alone insufficient under load |

---

## dongle-h753 — NUCLEO-H753ZI

**Branch:** `feature/stm32f753-bring-up` (squashed + merged)  
**Last Updated:** 2026-04-27

### Completed

- [x] Rust toolchain setup (rustup stable + thumbv7em-none-eabihf target)
- [x] Critical clock bug fix: `H753_64MHZ.clock_hz = 32_000_000` (firmware uses PLL2Q = 32 MHz, not 64 MHz)
- [x] Firmware compiles to release binary (~5.3 KB)
- [x] Instrumentation: USB boot logs, FDCAN init, error warnings, sequence tracking
- [x] .cargo/config.toml configured with probe-rs runner
- [x] probe-rs installed and ST-LINK V3 detected
- [x] **Phase 0:** Bench setup, wiring confirmed, linker flags fixed, flash/log workflow repeatable
- [x] **Phase 1:** USB enumerates as "KCAN Dongle v1" — VID 0x1209, PID 0xBEEF, 12 Mb/s (USB FS)
- [x] **Phase 4:** EP0 control plane: GET_INFO, GET_BT_CONST, SET_BITTIMING, SET_MODE all ACK'd
- [x] **Phase 5 partial:** Bulk IN working — 280 clean 80-byte KCAN frames in 20 s, zero Cancelled errors

### Pending

- [ ] Phase 2 — FDCAN external bus validation (internal loopback done; scope + second node needed)
- [ ] Phase 3 — Bulk OUT path test (host→device TX)
- [ ] Phase 6 — Reliability gating (soak, reconnect, bus-off recovery)

### Build Commands

```sh
cd firmware

# Normal mode
cargo run --release -p dongle-h753

# Dual-channel cross-test (FDCAN1 ↔ FDCAN2)
cargo run --release -p dongle-h753 --features bus-test

# FDCAN internal loopback self-test
cargo run --release -p dongle-h753 --features loopback

# Periodic echo on both channels (100 ms)
cargo run --release -p dongle-h753 --features periodic-echo

# Release binary only (no flash)
cargo build --release -p dongle-h753
```

### Host Config

```sh
cargo run --release -- --config host/config.kcan.json
```

### Phase 0: Bench Setup ✅

- [x] Confirm STM32H753ZI Nucleo board wiring (FDCAN1 pins PD0/PD1, USB FS PA11/PA12)
- [x] Verify CAN transceiver power and termination (120 Ω)
- [x] Lock repeatable flash/log procedure (defmt + probe-rs, analyzer baseline)
- [x] Define baseline instrumentation: USB enumeration logs, CAN RX/TX counters

**Hardware note:** SB149/SB150 solder bridges are open by default on Nucleo MB1137;
plug CN13 cable after firmware boots, or close the bridges permanently.

---

### Phase 1: USB Enumeration ✅

- [x] Flash firmware; defmt: "KCAN Dongle v1.0.0 — booting"
- [x] macOS `system_profiler SPUSBDataType` shows "KCAN Dongle v1", VID 0x1209, PID 0xBEEF
- [x] Plug/unplug cycles consistent; no USB errors in defmt

**Result:** Enumerates as "KCAN Dongle v1", Kodezine, KCAN0001, 12 Mb/s (USB FS — hardware limit of OTG FS peripheral; acceptable for CAN workloads).

---

### Phase 2: FDCAN Physical Validation 🔄

**Gate:** Observable RX/TX frames at 250 kbps on analyzer + scope; clock matches

- [x] Critical clock fix applied: `H753_64MHZ.clock_hz = 32_000_000` in [kcan-protocol/src/control.rs](../kcan-protocol/src/control.rs)
- [x] RTT confirms `pll2_q: MaybeHertz(32000000)` and "FDCAN1: INTERNAL LOOPBACK mode, 250 kbps"
- [x] FDCAN internal loopback self-test PASS — `[ID=0x123, DE AD BE EF ...]` TX/RX confirmed
- [ ] Scope trigger on CAN_RX pin (PD0) — confirm bus-idle high and transitions on frames
- [ ] Connect second CAN node; verify firmware RX ISR fires ("FDCAN RX [ID=0x123, DLC=8]")
- [ ] Confirm frame visible on analyzer at 250 kbps
- [ ] Test extended ID and RTR frames

---

### Phase 3: USB Bulk Data Plane 🔄

**Gate:** Bidirectional Bulk IN/OUT with correct timestamps and sequence numbers

- [x] Bulk IN (device→host): 280 clean 80-byte KCAN frames in 20 s, zero Cancelled errors
  - Three bugs fixed: BULK_RESTART deadlock, USB_CONFIGURED signal contention, EP1 TX FIFO underrun + DATA0 toggle mismatch (commit `b96ffa1`)
- [ ] Bulk OUT (host→device): send raw KCAN frames via Bulk OUT; observe TX on CAN bus
- [ ] Verify TX echo returned with correct sequence and timestamp
- [ ] Test sequence wrapping: >65536 frames, confirm seq 0xFFFF→0x0000 clean
- [ ] Burst test: 1000+ frames/sec from external node; measure drop rate (target: <1%)

---

### Phase 4: EP0 Control Plane ✅

- [x] `KCanEp0Handler` implemented in `firmware/dongle-h753/src/ep0_handler.rs`
- [x] GET_INFO, GET_BT_CONST, SET_BITTIMING, SET_MODE all ACK'd
- [x] `KCanAdapter::open()` at 250 kbps succeeds without error

---

### Phase 5: End-to-End Integration ✅ partial

- [x] Host adapter `.open()` at 250 kbps succeeds
- [x] 280 clean KCAN frames received by rustycan in 20 s
- [x] No session hangs or USB timeouts over 20-second run
- [ ] Send 100 frames bidirectionally (Bulk OUT path not yet exercised from host)

---

### Phase 6: Reliability Gating ⏳

- [ ] Soak: 5 minutes at 100–500 frames/sec; <0.1% unexpected drops
- [ ] Timestamps monotonic, sequence continuous over soak
- [ ] USB disconnect/reconnect (5× cycles) — no deadlock
- [ ] SET_MODE after re-plug — BULK_RESTART recovers cleanly
- [ ] Bus-Off recovery (short TX with no termination, then reconnect)
- [ ] Cold-boot re-enumeration within 2 seconds
- [ ] No hanging tasks or deadlocks in defmt over soak duration

---

### Phase 7: Deferred Enhancements

- [ ] Dynamic bitrate via SET_BITTIMING control request
- [ ] Status frame generation (FrameType::Status) with error counters
- [ ] LISTEN_ONLY and LOOPBACK mode implementations
- [ ] CAN FD and BRS frame support
- [ ] Timestamp epoch/offset for long-running systems (TIM2 rollover handling)

---

## dongle-h743 — STM32H743I-EVAL MB1246 Rev E

**Branch:** `feature/dongle-h743-bringup`  
**Status:** All phases pending — firmware compiles; hardware not yet connected

### Hardware Connector Map

| Connector | Function | Notes |
|-----------|----------|-------|
| C23 | ST-LINK V3E (Micro-USB) | Debug / flash |
| CN3 | CAN DB9 → TJA1044 transceiver | PH13 (TX) / PH14 (RX) ⚠️ confirm from schematic |
| CN18 | USB OTG FS Micro-AB | PA11 (DM) / PA12 (DP); JP2 must not be fitted |
| CN16 | OTG1 FS | not used |
| CN14 | OTG HS (ULPI) | not used |

> **⚠️ Action required before Phase 3:**  
> Confirm PH13 (TX) / PH14 (RX) against the MB1246-H743-E03 schematic
> (available on the ST product page, "Schematic Pack"). Update `main.rs` if
> the TJA1044 is wired differently.

### Build Commands

> **Important:** Run from the package directory so that
> `firmware/dongle-h743/.cargo/config.toml` (chip = `STM32H743XIHx`) takes
> precedence over the workspace-level config (chip = `STM32H753ZITx`).
> Running `cargo run -p dongle-h743` from `firmware/` will use the wrong chip.

```sh
cd firmware/dongle-h743

# Normal mode
cargo run --release

# FDCAN internal loopback self-test (Phase 2)
cargo run --release --features loopback

# Periodic echo on FDCAN1 @ 100 ms (Phase 3)
cargo run --release --features periodic-echo

# Release binary only (no flash)
cargo build --release
```

### Host Config

```sh
cargo run --release -- --config host/config.kcan-h743.json
```

### Phase 0: Bench Setup ✅

**Gate:** probe-rs detects the chip; defmt RTT output visible

- [x] Connect barrel jack power supply (5 V) to eval board
- [x] Connect Micro-USB cable: host Mac → **C23** (ST-LINK V3E)
- [x] `probe-rs list` enumerates STM32H743XIHx (detected as STLink V3)
- [x] `cargo build --release` (from `firmware/dongle-h743/`) — confirm `.elf` produced (already verified in CI)
- [x] Flash loopback build: `cargo run --release --features loopback` (from `firmware/dongle-h743/`)
- [x] Confirm defmt RTT output visible in terminal

**Result:** probe-rs detected, firmware flashed, RTT streaming. HSE=25 MHz, sysclk=480 MHz, pll2_q=32 MHz all confirmed in clock debug log.

---

### Phase 1: USB Enumeration ⏳

**Gate:** `system_profiler SPUSBDataType` shows VID=0x1209, PID=0xBEEF, "KCAN Dongle v1 (H743I)"

- [ ] Flash default build: `cargo run --release` (from `firmware/dongle-h743/`)
- [ ] Connect Micro-USB: host Mac → **CN18** (OTG FS); JP2 must not be fitted
- [ ] RTT: `KCAN Dongle v1 (H743I) — booting`
- [ ] `system_profiler SPUSBDataType` shows device
- [ ] VID=0x1209, PID=0xBEEF, serial = UID hex

**Note:** `vbus_detection = false` — CN18 cable may be pre-plugged; D+ pulled
high immediately at boot.

---

### Phase 2: FDCAN Loopback Self-Test ✅

**Gate:** RTT: `FDCAN self-test: PASS [ID=0x123, loopback RX matched TX]`

- [x] Flash: `cargo run --release --features loopback` (from `firmware/dongle-h743/`)
- [x] RTT: `FDCAN1: INTERNAL LOOPBACK mode, 250 kbps — Phase 2 self-test`
- [x] RTT: `FDCAN self-test: PASS`

**Result:** PLL2Q = 32 MHz confirmed. FDCAN1 bit timing correct at 250 kbps.

---

### Phase 3: FDCAN Physical Bus Validation ⏳

**Gate:** Frames visible on PEAK PCAN-USB sniffer or second CAN node

**Prerequisite:** Confirm PH13/PH14 from schematic (see ⚠️ above).

- [ ] Connect CAN cable: CN3 DB9 ↔ CAN analyser or second CAN node
- [ ] Ensure bus termination (120 Ω at each end)
- [ ] Flash: `cargo run --release --features periodic-echo` (from `firmware/dongle-h743/`)
- [ ] RTT: `echo TX FDCAN1 [ID=0x7E1, counter=N]` every 100 ms
- [ ] Sniffer sees 0x7E1 @ 250 kbps
- [ ] RTT: `FDCAN RX [ID=...]` when second node transmits

---

### Phase 4: EP0 Control Plane ⏳

**Gate:** `cargo run --release -- --config host/config.kcan-h743.json` connects without error

- [ ] Flash default build: `cargo run --release` (from `firmware/dongle-h743/`)
- [ ] Plug CN18 to Mac
- [ ] Run rustycan with H743 config
- [ ] RTT: `SET_MODE received — signalling BULK_RESTART`
- [ ] Host log: `KCan adapter opened at 250 kbps`

---

### Phase 5: Bulk IN/OUT Data Plane ⏳

**Gate:** rustycan GUI shows live frames from CN3; TX from GUI appears on sniffer

- [ ] Connect CAN node to CN3
- [ ] Run rustycan with H743 config
- [ ] Frame counter increments in GUI
- [ ] Send TX frame from GUI → confirm on sniffer
- [ ] Soak: 280+ frames over 20 s, zero Cancelled errors (mirrors dongle-h753 baseline)

---

### Phase 6: Reliability Gating ⏳

**Gate:** All recovery scenarios pass without firmware hang

- [ ] USB disconnect/reconnect (5× cycles) — no deadlock
- [ ] SET_MODE after re-plug — BULK_RESTART recovers cleanly
- [ ] Bus-Off recovery (short TX, no termination, then reconnect)
- [ ] Soak: 10 minutes continuous RX at 250 kbps, zero frame drops

---

## Shared Reference

### Key Files

| File | Responsibility | Phases |
|------|----------------|--------|
| [firmware/dongle-h753/src/main.rs](../firmware/dongle-h753/src/main.rs) | Board init, clocks, task spawn | 0–6 |
| [firmware/dongle-h743/src/main.rs](../firmware/dongle-h743/src/main.rs) | Board init, clocks, task spawn | 0–6 |
| [firmware/dongle-h753/src/kcan_usb.rs](../firmware/dongle-h753/src/kcan_usb.rs) | USB endpoint registration | 1, 4 |
| [firmware/dongle-h753/src/usb_task.rs](../firmware/dongle-h753/src/usb_task.rs) | Bulk IN/OUT data path | 3 |
| [firmware/dongle-h753/src/can_task.rs](../firmware/dongle-h753/src/can_task.rs) | FDCAN RX/TX, frame conversion | 2, 3 |
| [firmware/dongle-h753/src/ep0_handler.rs](../firmware/dongle-h753/src/ep0_handler.rs) | EP0 control requests | 4 |
| [kcan-protocol/src/control.rs](../kcan-protocol/src/control.rs) | Clock constant (32 MHz); BT const | 2, 4 |
| [host/src/adapters/kcan.rs](../host/src/adapters/kcan.rs) | Host control contract | 4, 5 |
| [host/src/session.rs](../host/src/session.rs) | End-to-end session flow | 5, 6 |

### Decision Log

1. **Clock correction (h753):** Firmware uses 32 MHz FDCAN core (PLL2Q). `control.rs` constant corrected from 64 → 32 MHz.
2. **Single channel (h743):** STM32H743I-EVAL has one physical CAN port (CN3). FDCAN2 and `bus-test` feature not implemented.
3. **USB_TO_CAN2 dummy (h743):** Kept as a static sink to satisfy `kcan_io_task` signature; host must not send channel-1 frames.
4. **First milestone for both:** 250 kbps fixed only; dynamic bitrate deferred.
5. **Pre-commit clippy:** Both firmware packages must be linted separately (`-p` flag) — stm32-metapac rejects two chip features in a single workspace invocation.

### Pass/Fail Record Template

```
[dongle-h7xx] [Phase X] — YYYY-MM-DD
Status: ✅ PASS / ❌ FAIL
Details:
- <checkpoint result>
- <defmt log snippet if relevant>
- <blocker if failed>
Next: Phase N
```
