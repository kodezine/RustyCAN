# STM32F753 USB-FDCAN Real-Hardware Bring-Up Plan

**Status:** Phases 4 & 5 (Bulk IN + Host Adapter) complete; Phase 2 external bus + Phase 3 Bulk OUT + Phase 6 Reliability pending  
**Branch:** `feature/stm32f753-bring-up`  
**Last Updated:** 2026-04-27

## ✅ Completed

- [x] Rust toolchain setup (rustup stable + thumbv7em-none-eabihf target)
- [x] Critical clock bug fix: `H753_64MHZ.clock_hz = 32_000_000` (firmware uses PLL2Q = 32 MHz, not 64 MHz)
- [x] Firmware compiles to release binary (~5.3K)
- [x] Instrumentation: USB boot logs, FDCAN init, error warnings, sequence tracking
- [x] .cargo/config.toml configured with probe-rs runner
- [x] probe-rs installed and ST-LINK V3 detected
- [x] **Phase 0 complete:** Bench setup, wiring confirmed, linker flags fixed, flash/log workflow repeatable
- [x] **Phase 1 complete:** USB enumerates as "KCAN Dongle v1" — VID 0x1209, PID 0xBEEF, 12 Mb/s (USB FS). All plug cycles consistent. Hardware note: CN13 cable must be plugged after firmware boots on Nucleo MB1137 (SB149/SB150 bridges open by default).
- [x] **Phase 4 complete:** EP0 control plane fully implemented (`ep0_handler.rs`): GET_INFO, GET_BT_CONST, SET_BITTIMING, SET_MODE all ACK'd; `KCanAdapter::open()` at 250 kbps succeeds without error.
- [x] **Phase 5 partial:** Bulk IN data plane working end-to-end — 280 clean 80-byte KCAN frames received by rustycan in 20 s with zero Cancelled errors after startup. Three firmware bugs fixed: BULK_RESTART deadlock, USB_CONFIGURED signal contention (status_task), EP1 TX FIFO underrun + DATA0 toggle mismatch (commit `b96ffa1`). Bulk OUT (host→device) path implemented but not yet tested.

## ⏳ Pending

- [ ] Phase 2 FDCAN Physical Validation (external node + scope — internal loopback done)
- [ ] Phase 3 Bulk OUT path test (host→device TX); Bulk IN ✅ verified (280 frames/20s)
- [ ] Phase 6 Reliability Gating (soak test, recovery scenarios)

## Overview

Establish a step-by-step integration path for USB + FDCAN on the STM32F753 Nucleo board, starting with a low-risk 250 kbps baseline and hardware first validation using a second CAN node + analyzer + scope.

## Scope

- **Included:** Firmware + host adapter validation together
- **First Milestone:** Fixed 250 kbps baseline (no dynamic bitrate)
- **Excluded from Milestone:** CAN FD, dynamic reconfiguration, multi-channel
- **Lab Tools:** CAN analyzer, oscilloscope/logic analyzer, second STM32/CAN node

## Execution Phases

### Phase 0: Bench Setup and Success Criteria ✅
**Goal:** Confirm stable hardware foundation and repeatable test workflow  
**Effort:** ~1 hour  
**Gate:** Defined pass/fail artifacts per later stage

- [x] Confirm STM32F753 Nucleo board wiring (FDCAN1 pins PD0/PD1, USB FS)
- [x] Verify CAN transceiver power and termination (120 Ω if applicable)
- [x] Lock repeatable flash/log procedure (defmt setup, probe-run, analyzer baseline)
- [x] Define baseline instrumentation: USB enumeration logs, CAN RX/TX counters, drop counters

**References:**
- [firmware/dongle-h753/src/main.rs](../firmware/dongle-h753/src/main.rs) — board config

---

### Phase 1: USB Device Bring-Up ✅
**Goal:** Prove reliable device enumeration and basic USB readiness  
**Effort:** ~2 hours  
**Depends on:** Phase 0  
**Gate:** Consistent enumeration as VID 0x1209 / PID 0xBEEF with expected endpoints

- [x] Flash firmware and verify defmt output: "KCAN Dongle v1.0.0 — booting"
- [x] On host OS, confirm macOS System Info shows device (equivalent to lsusb)
- [x] Add instrumentation logs for USB stack readiness and endpoint activity
- [x] Test plug/unplug cycles to confirm consistency; RTT enumeration logs captured

**Checkpoint:** Device never fails to enumerate; defmt shows no USB errors ✅  
**Result:** macOS enumerates "KCAN Dongle v1", Kodezine, KCAN0001, VID 0x1209 / PID 0xBEEF, 12 Mb/s (USB FS — hardware-limited by OTG FS peripheral; acceptable for CAN workloads).  
**Hardware note:** SB149/SB150 solder bridges are open by default on Nucleo MB1137; plug CN13 cable after firmware boots, or close the bridges permanently.

**References:**
- [firmware/dongle-h753/src/main.rs](../firmware/dongle-h753/src/main.rs) — USB init, Irqs binding
- [firmware/dongle-h753/src/kcan_usb.rs](../firmware/dongle-h753/src/kcan_usb.rs) — endpoint registration

---

### Phase 2: FDCAN Physical and Timing at 250 kbps
**Goal:** Validate FDCAN clock correctness and prove bus traffic on wire  
**Effort:** ~3–4 hours  
**Depends on:** Phase 1  
**Gate:** Observable RX/TX frames at 250 kbps on analyzer and scope; clock assumptions match observations

#### Clock Validation
- [x] **Critical Bug Fix (done in Phase 0):** [kcan-protocol/src/control.rs](../kcan-protocol/src/control.rs) constant updated:
  ```
  H753_64MHZ.clock_hz = 32_000_000  (was 64_000_000)
  ```
  **Reason:** Firmware configures PLL2Q = 32 MHz; control response must match for correct host BRP calculation

- [x] Verify firmware logs show: "FDCAN1 initialized to 250 kbps (PLL2Q = 32 MHz)"\
  **Result:** RTT confirms `pll2_q: MaybeHertz(32000000)` and "FDCAN1: INTERNAL LOOPBACK mode, 250 kbps"
- [x] **FDCAN internal loopback self-test PASS:** firmware transmits `[ID=0x123, DE AD BE EF ...]`, receives it back immediately — clock and bit-timing confirmed correct at 250 kbps
- [ ] Scope trigger on CAN_RX pin (PD0): should show bus-idle high, then transitions on frame activity\
  *(requires external hardware — do this during Bus Traffic Validation below)*

#### Bus Traffic Validation
- [ ] Connect second CAN node (e.g., another STM32 or PEAK adapter) to same bus
- [ ] Set external node to transmit standard frame (ID 0x123, 8 bytes: [0x01, 0x02, ...])
- [ ] Verify firmware RX ISR fires; defmt logs: "FDCAN RX [ID=0x123, DLC=8]"
- [ ] Verify from analyzer: frame visible on wire at correct bitrate
- [ ] Repeat with extended ID, RTR frames; confirm ID masking (standard: [10:0], extended: [28:0])

#### Loopback Verification
- [x] Enable internal loopback via `periodic-echo` feature — echo_task fires every 100 ms on FDCAN1 (0x7E1) and FDCAN2 (0x7E2); both TX and RX counters confirmed in defmt
- [ ] Send frame via Bulk OUT; expect RX ISR to trigger immediately
- [x] Verify timestamp_us captured and logs show RX counter increment — confirmed: "FDCAN RX [ID=0x000007e1, DLC=8]" and "FDCAN RX [ID=0x000007e2, DLC=8]" at correct rate

**Checkpoint:** All frame types (standard, extended, RTR) RX consistently; clock observable on scope matches expected 250 kbps timing

**References:**
- [firmware/dongle-h753/src/main.rs](../firmware/dongle-h753/src/main.rs) — clock config (lines ~150–165)
- [firmware/dongle-h753/src/can_task.rs](../firmware/dongle-h753/src/can_task.rs) — RX handling, frame conversion

---

### Phase 3: USB Bulk Data Plane
**Goal:** Prove bidirectional USB Bulk IN/OUT, frame conversion, and queue backpressure handling  
**Effort:** ~3–4 hours  
**Depends on:** Phase 1 and 2  
**Gate:** TX echo and RX appear in Bulk transfers with correct timestamps and sequence numbers

#### Host→Device Transmit Path (Bulk OUT)
- [ ] From host, send raw KCAN frames via Bulk OUT (80-byte format)
- [ ] Verify firmware parses: magic (0xCA), version (0x01), extracts CAN ID/DLC/data
- [ ] Observe TX frames appear on CAN bus (use analyzer)
- [ ] Verify TX echo returned to host with correct sequence and timestamp

#### Device→Host Receive Path (Bulk IN)
- [x] Inject CAN frames via `periodic-echo` feature (100 ms FDCAN self-echo on both channels)
- [x] Verify host receives Bulk IN frames: 280 clean 80-byte transfers in 20 s, zero Cancelled errors after startup
- [ ] Test sequence wrapping: send >65536 frames, confirm seq wraps 0xFFFF→0x0000 without corruption

**Result:** Three bugs fixed to achieve this: (1) BULK_RESTART deadlock, (2) USB_CONFIGURED signal stolen by status_task, (3) EP1 TX FIFO underrun + DATA0 toggle mismatch.  
See commit `b96ffa1` — "firmware: fix USB bulk IN deadlock and signal contention"

#### Backpressure and Loss Measurement
- [ ] Send burst (1000+ frames/sec) from external node
- [ ] Measure any dropped frames via defmt logs: "can_to_usb channel full — RX frame dropped"
- [ ] Record frame loss percentage; acceptable baseline: <1% at 1000 frames/sec
- [ ] Stress test over 30 seconds; confirm no USB hangs or timeouts

**Checkpoint:** All RX and TX frames traverse USB without loss; sequence continuous; timestamps monotonic

**References:**
- [firmware/dongle-h753/src/usb_task.rs](../firmware/dongle-h753/src/usb_task.rs) — Bulk IN/OUT handling
- [firmware/dongle-h753/src/can_task.rs](../firmware/dongle-h753/src/can_task.rs) — queue backpressure

---

### Phase 4: Minimum USB Control Plane (EP0) for Host Compatibility
**Goal:** Implement mandatory control requests so host adapter open succeeds  
**Effort:** ~3–5 hours  
**Depends on:** Phases 1, 2, 3  
**Gate:** Host `KCanAdapter::open()` succeeds without timeout or error

#### Required Control Requests
1. **GET_INFO (0x01):**
   - Host expects 12-byte response with fw_major, fw_minor, fw_patch, channels, protocol_version, uid_lo
   - Firmware MUST respond deterministically in <100 ms
   - **Action:** Implement control_in handler in [firmware/dongle-h753/src/main.rs](../firmware/dongle-h753/src/main.rs) or new `control_task.rs`

2. **GET_BT_CONST (0x06) (Optional but recommended):**
   - Return bit timing capability descriptor from [kcan-protocol/src/control.rs](../kcan-protocol/src/control.rs)
   - Ensure clock_hz matches actual firmware core (32 MHz, not 64 MHz)

3. **SET_MODE (0x04):**
   - Parse host request (BUS_ON, LISTEN_ONLY, LOOPBACK flags)
   - For first milestone: accept BUS_ON only; reject others gracefully
   - Transition FDCAN1 mode and capture TIM2 offset if applicable

#### Implementation Checklist
- [x] Add USB control handler — implemented in `firmware/dongle-h753/src/ep0_handler.rs` (`KCanEp0Handler`)
- [x] Register handler with `usb::Builder` — `builder.handler(ep0)` in `main.rs`
- [x] Test via host adapter open() — confirmed in defmt: GET_INFO → GET_BT_CONST → SET_BITTIMING → SET_MODE all ACK'd
- [x] All responses are deterministic and sub-100 ms — verified in practice
- [x] Control requests logged at INFO level in defmt

**Checkpoint:** ✅ Host opens without timeout; firmware logs all four control requests  
**Result:** `KCanAdapter::open()` at 250 kbps succeeds reliably

**References:**
- [host/src/adapters/kcan.rs](../host/src/adapters/kcan.rs) (lines ~50–110) — host control expectations
- [kcan-protocol/src/control.rs](../kcan-protocol/src/control.rs) — request/response format

---

### Phase 5: End-to-End Host Adapter Open + Smoke Tests
**Goal:** Validate complete integration with host session layer  
**Effort:** ~2–3 hours  
**Depends on:** Phase 4  
**Gate:** Host adapter opens cleanly; bidirectional frames flow through [host/src/session.rs](../host/src/session.rs)

- [x] Verify host adapter `.open()` at 250 kbps succeeds — confirmed; GET_INFO/GET_BT_CONST/SET_BITTIMING/SET_MODE sequence completes without error
- [x] Verify device→host frame reception: 280 clean KCAN frames received by rustycan reader_thread in 20 s
- [x] Check host logs for frame reception — `frame_tx` channel populated; no parse errors, no Cancelled
- [x] Verify no session hangs or USB timeouts — 20-second run clean after startup artifacts
- [ ] Send 100 frames bidirectionally (host→device Bulk OUT TX path not yet exercised)

**Checkpoint:** ✅ Host adapter opens cleanly; Bulk IN frames flow for full session duration  
**Partial:** Bulk OUT (host→device CAN TX) path implemented in firmware but not yet exercised from host

**References:**
- [host/src/adapters/kcan.rs](../host/src/adapters/kcan.rs) — `open()` and session contract
- [host/src/session.rs](../host/src/session.rs) — end-to-end session handling

---

### Phase 6: Reliability and Regression Gate
**Goal:** Prove stable operation under stress and recovery scenarios  
**Effort:** ~4–5 hours  
**Depends on:** Phase 5  
**Gate:** Soak traffic shows <0.1% unexpected drops; recovery from unplug/reset is clean and deterministic

#### Soak Test
- [ ] Send sustained CAN traffic (100–500 frames/sec) for 5 minutes
- [ ] Monitor defmt for unexpected drops or errors
- [ ] Confirm timestamps remain monotonic and sequence continuous
- [ ] Measure actual bitrate on analyzer (should be stable 250 kbps ±1%)

#### Unplug/Reset Recovery
- [ ] Board cold-boot (plug/unplug cycle); verify re-enumeration within 2 seconds
- [ ] USB reset from host; confirm device recovers and logs show "KCAN Dongle v1.0.0 — booting"
- [ ] CAN bus transient (transceiver powered off/on); confirm firmware recovers to RX state

#### Release Readiness Criteria
- [ ] Soak: <0.1% unexpected frame loss in 5-minute run at 500 frames/sec
- [ ] Recovery: all unplug/reset cycles result in clean re-enumeration
- [ ] No hanging tasks or deadlocks in defmt logs over soak duration
- [ ] Scope captures confirm consistent bit timing (no phase/jitter drift)

**Checkpoint:** Ready to declare hardware-supported release for 250 kbps baseline

---

### Phase 7: Deferred Enhancements (Out of Milestone)
**Effort:** TBD  

- [ ] Dynamic bitrate via SET_BITTIMING control request
- [ ] Status frame generation (FrameType::Status) with error counters
- [ ] LISTEN_ONLY and LOOPBACK mode implementations
- [ ] CAN FD and BRS frame support
- [ ] Timestamp epoch/offset for long-running systems (TIM2 rollover handling)

---

## Key Files and Responsibilities

| File | Responsibility | Phases |
|------|-----------------|--------|
| [firmware/dongle-h753/src/main.rs](../firmware/dongle-h753/src/main.rs) | Board init, USB/FDCAN clock config, task spawn, control handler (new) | 0–6 |
| [firmware/dongle-h753/src/kcan_usb.rs](../firmware/dongle-h753/src/kcan_usb.rs) | USB endpoint registration | 1, 4 |
| [firmware/dongle-h753/src/usb_task.rs](../firmware/dongle-h753/src/usb_task.rs) | Bulk IN/OUT data path | 3 |
| [firmware/dongle-h753/src/can_task.rs](../firmware/dongle-h753/src/can_task.rs) | FDCAN RX/TX, frame conversion, seq/timestamp | 2, 3 |
| [firmware/dongle-h753/src/status_task.rs](../firmware/dongle-h753/src/status_task.rs) | Status/LED (deferred enhancement) | 7 |
| [kcan-protocol/src/control.rs](../kcan-protocol/src/control.rs) | Clock constant fix (32 MHz, not 64 MHz) | 2, 4 |
| [host/src/adapters/kcan.rs](../host/src/adapters/kcan.rs) | Host control contract validation | 4, 5 |
| [host/src/session.rs](../host/src/session.rs) | End-to-end session flow | 5, 6 |

---

## Decision Log

1. **Clock correction:** Firmware uses 32 MHz FDCAN core (PLL2Q), but constant says 64 MHz; update `control.rs` to match reality.
2. **First milestone:** 250 kbps only; dynamic bitrate deferred to post-milestone.
3. **Control plane minimum:** GET_INFO and SET_MODE handled; GET_BT_CONST optional for Phase 2.
4. **Hardware-first:** Validate clock and bus timing **before** host integration to catch timing mismatches early.

---

## Notes for Day-to-Day Work

- **Daily check-in:** Run Phase 0 baseline defmt + analyzer snapshot to confirm no regressions
- **Blocker escalation:** If any phase hangs >5 minutes, check defmt for "channel full" or "USB timeout" messages
- **Instrumentation:** Add periodic (every 1 min) debug log: `[USB: %d RX %d TX] [CAN: %d RX %d TX %d drops]`
- **Bench setup:** Keep external CAN node running continuously during Phases 2–6 for reproducibility

---

## Pass/Fail Summary Template

```
[Phase X] — [Date/Time]
Status: ✅ PASS / ❌ FAIL
Details:
- [Specific checkpoint result]
- [Defmt log snippet if relevant]
- [Blocker if failed]
Next: [Phase N]
```
