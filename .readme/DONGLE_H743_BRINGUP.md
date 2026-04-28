# STM32H743I-EVAL USB-FDCAN Bring-Up Plan

**Board:** STM32H743I-EVAL, MB1246 Rev E (STM32H743XI, TFBGA240+25)  
**Branch:** `feature/dongle-h743-bringup`  
**Status:** Phase 0 — not yet started  
**Note:** Remove this file once hardware bring-up is complete.

## Board Overview

| Item | Detail |
|------|--------|
| MCU | STM32H743XI (same core as H753ZI, no crypto) |
| Board | MB1246 Rev E |
| Debugger | ST-LINK V3E — **C23** (Micro-USB) |
| USB OTG FS | **CN18** Micro-AB — PA11/PA12 |
| CAN DB9 | **CN3** — on-board TJA1044 → PH13 (TX) / PH14 (RX) |
| Power | Barrel jack (5 V) recommended; CN18 VBUS alone insufficient under load |
| LEDs | LD1 (green, PF10), LD3 (orange, PA4); LD2/LD4 behind I2C IO expander |

## Key Differences from dongle-h753

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
| USB OTG FS | PA11/PA12, CN13 | PA11/PA12, CN18 |
| Heartbeat LED | PB0 (green) | PF10 (green) |
| USB status LED | PE1 (blue) | PA4 (orange) |

## Pending Hardware Confirmation

> **Action required before flashing normal mode:**  
> Confirm FDCAN1 pin assignments from the MB1246-H743-E03 schematic
> (available on the ST product page under "Schematic Pack").  
> Plan assumes **PH13 (TX) / PH14 (RX)** for CN3.  
> If the TJA1044 transceiver is wired differently, update `main.rs` before Phase 3.

## Scope

- **Included:** Single-channel FDCAN1 via on-board TJA1044 transceiver (CN3 DB9)
- **First Milestone:** Fixed 250 kbps, USB FS enumeration, Bulk IN/OUT verified
- **Excluded:** FDCAN2 (single physical CAN port), bus-test feature, CAN FD

## Execution Phases

### Phase 0: Bench Setup ⏳
**Goal:** Confirm wiring, power, and flash/log workflow  
**Gate:** probe-rs detects the chip; defmt RTT output visible

- [ ] Connect barrel jack power supply (5 V) to eval board
- [ ] Connect Micro-USB cable: host Mac → **C23** (ST-LINK V3E) for debug
- [ ] Verify `probe-rs list` enumerates the board as STM32H743XIHx
- [ ] Run `cargo build --release --package dongle-h743` — confirm `.elf` produced
- [ ] Flash loopback build: `cargo run --package dongle-h743 --features loopback`
- [ ] Confirm defmt RTT output visible in terminal

**References:**
- [firmware/dongle-h743/src/main.rs](../firmware/dongle-h743/src/main.rs)
- [firmware/dongle-h743/.cargo/config.toml](../firmware/dongle-h743/.cargo/config.toml)

---

### Phase 1: USB Enumeration ⏳
**Goal:** Host Mac enumerates the device as "KCAN Dongle v1 (H743I)"  
**Gate:** `system_profiler SPUSBDataType` shows VID=0x1209, PID=0xBEEF

- [ ] Flash default build (no features): `cargo run --package dongle-h743`
- [ ] Connect Micro-USB cable: host Mac → **CN18** (OTG FS)
  - JP2 must **not** be fitted
- [ ] Confirm RTT log: `KCAN Dongle v1 (H743I) — booting`
- [ ] Confirm `system_profiler SPUSBDataType` shows device
- [ ] Confirm `nusb` (or `lsusb`) sees VID=0x1209, PID=0xBEEF, serial = UID hex

**Hardware note:** With `vbus_detection = false`, CN18 cable may be pre-plugged;
D+ is pulled high immediately at boot.

---

### Phase 2: FDCAN Loopback Self-Test ⏳
**Goal:** Verify PLL2Q = 32 MHz and FDCAN bit timing without external hardware  
**Gate:** RTT log prints `FDCAN self-test: PASS [ID=0x123, loopback RX matched TX]`

- [ ] Flash loopback build: `cargo run --package dongle-h743 --features loopback`
- [ ] Confirm RTT: `FDCAN1: INTERNAL LOOPBACK mode, 250 kbps — Phase 2 self-test`
- [ ] Confirm RTT: `FDCAN self-test: PASS`
- [ ] If FAIL with timeout → verify PLL2Q configuration (25 MHz HSE, prediv=5, mul=64, divq=10)

---

### Phase 3: FDCAN Physical Bus Validation ⏳
**Goal:** Send and receive a real CAN frame on the CN3 DB9 connector  
**Gate:** Frames visible on PEAK PCAN-USB sniffer or second CAN node

> **Prerequisite:** Confirm PH13/PH14 pin assignment against MB1246-E03 schematic.

- [ ] Connect CAN cable: CN3 DB9 ↔ CAN analyser or second CAN node
- [ ] Ensure bus termination (120 Ω at each end)
- [ ] Flash periodic-echo: `cargo run --package dongle-h743 --features periodic-echo`
- [ ] Confirm RTT: `echo TX FDCAN1 [ID=0x7E1, counter=N]` every 100 ms
- [ ] Confirm sniffer sees 0x7E1 @ 250 kbps on the bus
- [ ] Confirm RTT: `FDCAN RX [ID=...]` when second node transmits

---

### Phase 4: EP0 Control Plane ⏳
**Goal:** Host KCAN adapter opens the dongle and completes the GET_INFO / SET_MODE handshake  
**Gate:** `rustycan --config config.kcan-h743.json` connects without error

- [ ] Flash default build: `cargo run --package dongle-h743`
- [ ] Plug CN18 cable to Mac
- [ ] Run: `cargo run --release -- --config host/config.kcan-h743.json`
- [ ] Confirm RTT: `SET_MODE received — signalling BULK_RESTART`
- [ ] Confirm host log: `KCan adapter opened at 250 kbps`

---

### Phase 5: Bulk IN/OUT Data Plane ⏳
**Goal:** Live CAN frames flow from bus → host GUI  
**Gate:** rustycan GUI shows live frames from CN3

- [ ] Connect CAN node to CN3
- [ ] Run rustycan with H743 config
- [ ] Confirm frame counter increments in GUI
- [ ] Send a TX frame from GUI → confirm on sniffer
- [ ] Soak: 280+ frames over 20 s, zero Cancelled errors (mirrors dongle-h753 baseline)

---

### Phase 6: Reliability Gating ⏳
**Goal:** Stable operation through reconnect and edge-case scenarios  
**Gate:** All scenarios pass without firmware hang

- [ ] USB disconnect/reconnect (5× cycles) — no deadlock
- [ ] SET_MODE after re-plug — BULK_RESTART recovers cleanly
- [ ] Bus-Off recovery (short TX with no termination, then reconnect)
- [ ] Soak: 10-minute continuous RX at 250 kbps, zero frame drops

---

## Build Commands

```sh
# Default (normal mode)
cargo run --package dongle-h743

# Phase 2 loopback self-test
cargo run --package dongle-h743 --features loopback

# Phase 3 periodic echo (FDCAN1 only)
cargo run --package dongle-h743 --features periodic-echo

# Release binary (no flash)
cargo build --release --package dongle-h743
```

## Host Config

```sh
cargo run --release -- --config host/config.kcan-h743.json
```
