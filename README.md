# RustyCAN

[![Release](https://github.com/kodezine/RustyCAN/actions/workflows/release.yml/badge.svg)](https://github.com/kodezine/RustyCAN/actions/workflows/release.yml)
[![Latest release](https://img.shields.io/github/v/release/kodezine/RustyCAN)](https://github.com/kodezine/RustyCAN/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey)](#installation)

A native cross-platform GUI for monitoring, decoding, and controlling CANopen networks.
Connect a **PEAK PCAN-USB** adapter or a **KCAN Dongle** (STM32H753ZI-based,
built with Embassy), optionally provide EDS device-description files, and get
live NMT state, PDO signal values, and SDO transactions — all stored to a
newline-delimited JSON log.

The **KCAN Dongle** is the project's own first-class hardware target: a custom
USB CAN adapter with hardware timestamps, a fully documented binary protocol,
and a path to Phase 3 hardware-level encryption (STM32H563, TrustZone).

## Installation

| Platform | Artifact | Guide |
|---|---|---|
| 🍎 **macOS** (Apple Silicon & Intel) | `.dmg` · Homebrew tap | [install-macos.md](.readme/install-macos.md) |
| 🪟 **Windows** (x86-64) | NSIS installer `.exe` | [install-windows.md](.readme/install-windows.md) |
| 🐧 **Linux** (x86-64) | AppImage · `.deb` | [install-linux.md](.readme/install-linux.md) |

> **Quick install on macOS:**
> ```sh
> brew tap kodezine/rustycan && brew install --cask rustycan
> ```

[📦 All release artifacts →](https://github.com/kodezine/RustyCAN/releases/latest)

## ✨ Features

| Feature | Details |
|---|---|
| 🖥️ **Native GUI** | egui/eframe window — no terminal required |
| 🔌 **Adapter selection** | Choose PEAK PCAN-USB or KCAN Dongle from the Connect screen |
| 🔧 **KCAN Dongle** | STM32H753ZI Nucleo firmware (Embassy); custom 80-byte USB protocol with hardware timestamps |
| ⏱️ **Hardware timestamps** | KCAN frames carry µs-precision timestamps from FDCAN TIM2; logged as `hw_ts_us` in JSONL |
| 🔍 **Dongle detection** | Connect button enabled only when the selected adapter is found; re-checked every 2 s || 🔄 **Automatic adapter fallback** | If configured adapter unavailable, automatically tries other types (PEAK ↔ KCAN) with notice || � **Error resilience** | Continues running through adapter I/O errors; useful for waiting on unpowered buses or temporary disconnections |
| �👂 **Listen-only mode** | Optional passive mode — no frames are ever transmitted; toggle at connect time |
| � **Configuration persistence** | Form settings (port, baud, nodes, DBC files) saved to JSON and restored on next launch; missing files filtered out |
| �📄 **EDS optional** | Per-node EDS files are optional; PDO frames without EDS show raw byte values |
| 🆔 **Node ID from EDS** | Browsing to an EDS file auto-fills the Node ID from `[DeviceComissioning] NodeId` |
| 🌐 **Multi-node** | Configure any number of CANopen nodes; nodes are optional when using DBC-only monitoring |
| 💓 **NMT monitoring** | Live Bootup / Pre-Operational / Operational / Stopped state per node with age |
| 📡 **NMT commands** | Send Start / Stop / Enter Pre-Op / Reset Node / Reset Comm to any node or broadcast all |
| 📊 **PDO live values** | Decode TPDO/RPDO signals from EDS mappings; raw hex bytes when no EDS is loaded |
| � **DBC signal decoding** | Load a `.dbc` file to decode any message/signal on the bus; runs in dual-mode alongside CANopen; Intel & Motorola byte orders, VAL_ descriptions — [details](.readme/dbc-signal-decoding.md) |
| �🔎 **SDO decode** | Expedited upload/download with EDS name lookup; abort codes displayed |
| 📶 **Bus load bar** | 20-block colour-coded bar in the status strip: blue ≤30 %, yellow 30–70 %, red >70 % |
| 🎞️ **Frame rate** | Rolling fps counter (2 s window) shown alongside total frame count |
| 📝 **JSONL logging** | Every event (received and sent) appended to a newline-delimited JSON file |

## 🔌 Prerequisites

### Hardware

RustyCAN supports two adapters; at least one is required.

#### Option A — PEAK PCAN-USB

A **PEAK PCAN-USB** adapter with the macOS PCANBasic library:

1. Download the latest *PCUSB* `.pkg` from **<https://mac-can.com>**.
2. Run the installer — it places `libPCBUSB.dylib` in `/usr/local/lib/`.
3. Connect your PCAN-USB adapter; it appears as channel `1` by default.

#### Option B — KCAN Dongle (STM32H753ZI Nucleo)

The KCAN Dongle is the project’s own hardware target. Flash the Embassy
firmware onto a **NUCLEO-H753ZI** board:

```sh
# Install the target and flashing tool
rustup target add thumbv7em-none-eabihf
cargo install probe-rs-tools

# Build and flash
cd firmware
cargo run --release -p dongle-h753
```

Connect the board’s **CN5 Micro-B USB** port to the host; it enumerates as
VID `0x1209` / PID `0xBEEF` (“KCAN Dongle v1”). Wiring for CAN:

| Nucleo pin | Signal | TJA1051T |
|------------|--------|----------|
| PD0 | FDCAN1 RX | RXD |
| PD1 | FDCAN1 TX | TXD |
| 3V3 | VCC | VCC |
| GND | GND | GND |

### 🦀 Rust toolchain

```sh
rustup update stable  # MSRV: 1.85+
```

## 🚀 Build & run

```sh
git clone https://github.com/kodezine/RustyCAN
cd RustyCAN
cargo run --release -p rustycan
```

The GUI window opens immediately. No command-line flags are required.

## 🖥️ Using the GUI

### Connect screen

```
┌─ Connection ──────────────────────────────────────────────┐
│  Adapter:  ○ PEAK PCAN-USB   ● KCAN Dongle ★              │
│  KCAN:     [ KCAN Dongle v1.0.0 (SN: 00000001) ▼ ]        │
│  Port:     [ 1 ]  (hidden for KCAN)                       │
│  Baud:     [ 250000 ▼ ]  ● Dongle: Connected              │
│  Log:      [ rustycan.jsonl              ] [Browse…]      │
├─ Nodes ───────────────────────────────────────────────────┤
│  Node ID   EDS file path                                  │
│  [ 1     ] [ /path/to/motor.eds    ] [Browse…] [✕]        │
│  [ 2     ] [                       ] [Browse…] [✕]        │
│                                      [+ Add node]         │
├───────────────────────────────────────────────────────────┤
│                              [  Connect  ]  (greyed out   │
│                               until dongle found)         │
└───────────────────────────────────────────────────────────┘
```

**Adapter** — radio buttons to choose between PEAK PCAN-USB and KCAN Dongle.
The KCAN row (★) is the recommended first-class option.  
**KCAN device** — dropdown listing all KCAN dongles found via USB enumeration
(VID `0x1209` / PID `0xBEEF`). Re-enumerated every 2 s.  
**Port** — PCAN-USB channel number (typically `1`); hidden when KCAN is selected.  
**Baud rate** — drop-down: 125000 / 250000 / 500000 / 1000000 bps.  
**Log file** — base name for the `.jsonl` output; defaults to `rustycan.jsonl`.
A timestamp (`YYYYMMDDHHMMSS`) is automatically inserted before the extension,
so `log.jsonl` creates `log_20263003130458.jsonl`. This ensures each run
produces a unique log file.  
**Dongle indicator** — polled every 2 s; the Connect button stays disabled
(greyed) until the adapter is found on the given port/baud.  
**Automatic adapter fallback** — if the configured adapter is not found during
probing, RustyCAN automatically tries other available adapter types (PEAK ↔ KCAN).
When a fallback succeeds, a blue notice appears: "⚠ PEAK PCAN-USB not found,
automatically switched to KCAN Dongle". Manually switching adapters clears the notice.  
**Nodes** — CANopen nodes to monitor (optional):
- *Node ID* — decimal (`5`) or hex with `0x`/`H` prefix/suffix
  (`0x05`, `05H`); valid range 1–127.
- *EDS file* — optional; click **Browse…** to pick a file. If the EDS
  contains `[DeviceComissioning] NodeId`, the Node ID box is pre-filled
  automatically. Leave the EDS blank to monitor the node without decoding.
- CANopen nodes are entirely optional — you can connect with zero nodes
  when using DBC signal decoding or raw frame logging only.

**DBC Nodes** — DBC files for signal decoding (optional, see [DBC details](.readme/dbc-signal-decoding.md)):
- Load multiple DBC files; first-match precedence for overlapping CAN IDs.
- Runs in dual-mode alongside CANopen.

**Configuration persistence** — all settings are saved to
`~/Library/Application Support/RustyCAN/config.json` (macOS) when you
successfully connect, and automatically restored on next launch. Files that
no longer exist are silently removed from the restored configuration.

### Monitor screen

**Error resilience** — RustyCAN continues running even when the adapter
encounters I/O errors (e.g., USB timeouts, temporary disconnections). This
allows you to:
- Connect to the adapter before the CAN bus is powered
- Wait for nodes to come online without manual reconnection
- Tolerate temporary USB communication issues

Errors are logged to the terminal (stderr) for debugging. Use the **Disconnect**
button to manually return to the Connect screen when needed.

```
 RustyCAN  ·  Port 1  ·  250000 bps  ·  2 node(s)              [Disconnect]
┌─ NMT Status ──────────────────────────────────────────────────────────────┐
│ Broadcast: [Start] [Stop] [Pre-Op] [Reset] [Reset Comm]                   │
│ Node  EDS            State           Last seen  Actions                   │
│    1  motor.eds      OPERATIONAL     0.3s ago   [Start][Stop]…            │
│    2  (no EDS)       PRE-OPERATIONAL 1.1s ago   [Start][Stop]…            │
├─ PDO Live Values ─────────────────────────────────────────────────────────┤
│ Node  PDO  Signal              Value       Updated                        │
│    1    1  StatusWord          39          0.05s ago                      │
│    1    1  VelocityActualValue 1234        0.05s ago                      │
│    2    1  Byte0               [AB]        0.12s ago                      │
│    2    1  Byte1               [CD]        0.12s ago                      │
├─ SDO Log (last 50) ───────────────────────────────────────────────────────┤
│ [12:01:01.234] N01 READ  6040h/00 ControlWord = 15                        │
│ [12:01:01.456] N01 WRITE 6040h/00 ControlWord = 15                        │
│  234.0 fps  Total: 45231  │  Bus [██████░░░░░░░░░░░░░░] 28.4%  │  📄 log  │
└───────────────────────────────────────────────────────────────────────────┘
```

**NMT Status** — one row per node. States are colour-coded:
- 🟢 `OPERATIONAL`
- 🟡 `PRE-OPERATIONAL`
- 🔴 `STOPPED`
- 🔵 `BOOTUP`

*Broadcast* strip — five buttons that send the NMT command to **all nodes**
(`target_node = 0x00`).  
*Actions* column — the same five buttons per row, addressed to **that node only**.  
State transitions in the table are confirmed by the next incoming heartbeat;
there is no optimistic update.

**PDO Live Values** — signals decoded from EDS mappings. For nodes without an
EDS the raw frame bytes appear as `Byte0`, `Byte1`, … in hex.

**SDO Browser** — located in the Monitor screen; click any row in the EDS-driven
table to select it and reveal Read/Write action buttons below. The entire row
(all columns) is clickable. Values are displayed in both decimal and hex format
for easy debugging (e.g., `42 [0x2A]`). String values show the readable text
followed by hex bytes in italics (e.g., `"Device Name" 44 65 76...`).

**SDO Log** — scrollable ring buffer (last 50 entries). `READ` entries are
coloured cyan; `WRITE` entries magenta. Abort codes appear in red. Values are
displayed with both their primary format and hex/ASCII representation.

**Status bar** — three items separated by dividers:
- **fps + total** — rolling frames/second (2 s window) and cumulative frame count.
- **Bus load bar** — 20 block characters (`█`/`░`) colour-coded by zone: blue for
  ≤ 30 %, yellow for 30–70 %, red for > 70 %. The percentage after the bar matches
  the colour of the highest zone reached. Load is estimated as
  `fps × 125 bits ÷ baud_rate × 100` (standard 11-bit ID, 8-byte frame with
  overhead/stuffing). Hover the bar to see the formula.
- **Log path** — filename of the active `.jsonl` log; hover to see the full path.

## 📝 JSONL log format

Each line is a self-contained JSON object flushed immediately to disk.
All CAN data bytes are written as `"0x##"` hex strings.

```jsonl
{"ts":"2026-03-28T12:01:01.234Z","type":"NMT_STATE","cob_id":"0x720","node":32,"state":"PRE-OPERATIONAL","raw":["0x7F"]}
{"ts":"2026-03-28T12:01:01.235Z","type":"SDO_READ","cob_id":"0x5A0","node":32,"index":"0x3000","subindex":"0x01","name":"Status Word","value":255,"raw":["0x4B","0x00","0x30","0x01","0xFF","0x00","0x00","0x00"]}
{"ts":"2026-03-28T12:01:01.240Z","type":"SDO_READ","cob_id":"0x5A0","node":32,"index":"0x1008","subindex":"0x00","name":"Device Name","value":[84,67,45,77,78,50,48,56,54,52,55,52,45,48,48,0],"ascii":"TC-MN2086474-00","raw":["0x43","0x08","0x10","0x00","0x54","0x43","0x2D","0x4D"]}
{"ts":"2026-03-28T12:01:01.300Z","type":"PDO","cob_id":"0x201","node":32,"pdo_num":1,"signals":{"Status Word":43,"Digital Inputs":0,"Current Segment Index":0},"raw":["0x2B","0x00","0x00","0x00"]}
{"ts":"2026-03-28T12:01:01.400Z","type":"NMT_COMMAND","cob_id":"0x000","command":"START","target_node":0,"raw":["0x01","0x00"]}
{"ts":"2026-03-28T12:01:01.401Z","type":"NMT_COMMAND_SENT","cob_id":"0x000","command":"START","target_node":1,"raw":["0x01","0x01"]}
```

### Common fields

| Field | Present in | Description |
|---|---|---|
| `ts` | all | ISO 8601 timestamp with millisecond precision |
| `type` | all | Entry type (see table below) |
| `cob_id` | all | CAN Object Identifier as `"0xNNN"` hex string |
| `raw` | all | Full CAN data bytes as `["0x##", …]` hex strings |
| `hw_ts_us` | KCAN only | Hardware timestamp in microseconds from FDCAN TIM2 (absent for PEAK frames) |

### Entry types

| `type` | Trigger | Key fields |
|---|---|---|
| `NMT_STATE` | Heartbeat or bootup frame received | `node`, `state` |
| `NMT_COMMAND` | NMT command frame observed on bus | `command`, `target_node` |
| `NMT_COMMAND_SENT` | NMT command sent by RustyCAN | `command`, `target_node` |
| `SDO_READ` | SDO upload response decoded | `node`, `index` (hex), `subindex` (hex), `name`, `value`, `ascii` (optional) |
| `SDO_WRITE` | SDO download request decoded | `node`, `index` (hex), `subindex` (hex), `name`, `value`, `ascii` (optional) |
| `PDO` | TPDO or RPDO frame decoded | `node`, `pdo_num` (EDS-derived), `signals` (name→value map) |
| `DBC_SIGNAL` | CAN frame decoded by loaded DBC | `message`, `source_dbc`, `signals` (name→{raw, physical, unit, description}) |
| `RAW_FRAME` | Unmatched frame (fallback) | `cob_id`, `raw` (no higher-level decode) |

**DBC note:** For decoded DBC signal JSONL contract details, see
[`.readme/dbc-signal-decoding.md`](.readme/dbc-signal-decoding.md#jsonl-logging-format-for-decoded-dbc-signals).
Multi-DBC loading is supported; multiple files are merged with first-match precedence for overlapping CAN IDs.

**RAW_FRAME note:** Only emitted for frames that were not logged as NMT/SDO/PDO/DBC_SIGNAL.
Prevents duplicate logging while capturing all bus traffic.

**SDO notes:**
- `ascii` — present only for byte array values (VISIBLE_STRING, OCTET_STRING) when
  all bytes are printable ASCII (0x20–0x7F), common whitespace characters (tab,
  newline, carriage return), or null terminators. Trailing nulls are stripped
  from the `ascii` string. Makes it easy to read string values directly in the
  log without manual decoding.

**PDO notes:**
- `node` and `pdo_num` are resolved from the EDS mapping for the matching COB-ID; if no EDS is loaded for the sending node, `node` is derived from the COB-ID range and signals fall back to `{"Byte0": "0x##", …}`.
- `signals` preserves EDS declaration order; typed values match the EDS `DataType` (integer, unsigned, float, or string).

## 🗂️ Project structure

```
Cargo.toml          root workspace (host, kcan-protocol)
kcan-protocol/      shared wire-protocol crate (no_std + std feature)
  src/
    frame.rs        KCanFrame — 80-byte wire format, LE, to_bytes/from_bytes
    control.rs      KCanBitTiming, KCanBtConst, KCanDeviceInfo, KCanMode, RequestCode
    encrypted.rs    EncryptionLayer trait stub (Phase 3 — STM32H563 SAES)
host/               rustycan host application
  Cargo.toml
  src/
    lib.rs          public library surface
    main.rs         binary entry-point (launches GUI)
    app.rs          AppState, CanEvent enum, event application logic
    session.rs      SessionConfig, CanCommand, adapter lifecycle, recv thread
    logger.rs       EventLogger — JSONL line writer (hw_ts_us for KCAN)
    adapters/
      mod.rs        CanAdapter trait, ReceivedFrame, AdapterKind, open_adapter
      peak.rs       PeakAdapter — wraps host_can (PCAN-USB)
      kcan.rs       KCanAdapter — nusb background-thread USB adapter
    eds/
      mod.rs        EDS INI parser; parse_node_id, parse_node_id_str
      types.rs      ObjectDictionary, OdEntry, DataType, AccessType
    canopen/
      mod.rs        COB-ID classification (classify_frame / extract_cob_id)
      nmt.rs        NMT decode (heartbeat, command) + encode_nmt_command
      sdo.rs        Expedited SDO decode (upload / download / abort)
      pdo.rs        PdoDecoder built from EDS TPDO/RPDO mapping objects
    gui/
      mod.rs        egui application — Connect & Monitor screens
  tests/
    integration_test.rs   end-to-end EDS + PDO + SDO + NMT tests
    fixtures/
      sample_drive.eds    CiA 402 servo drive test fixture
firmware/           separate Cargo workspace (embedded target)
  Cargo.toml        firmware workspace (dongle-h753)
  memory.x          STM32H753ZI memory map (FLASH 2 MB, DTCM 128 KB, AXI 512 KB)
  dongle-h753/
    src/
      main.rs       Embassy entry point, clock config (PLL1/2/3), task spawning
      kcan_usb.rs   KCanUsbClass — bulk IN/OUT endpoint pair (vendor class)
      can_task.rs   FDCAN1 RX/TX task (Frame ↔ KCanFrame conversion)
      usb_task.rs   USB device task + bulk IN/OUT bridge
      status_task.rs LED heartbeat + periodic STATUS frame every 100 ms
```

## 🧪 Testing

```sh
cargo test -p rustycan
```

77 unit tests + 11 integration tests covering the EDS parser, node-ID string
parsing, PDO bit extraction, SDO command-specifier decode, NMT encode/decode,
and the COB-ID classifier.

To verify the firmware crate type-checks (requires the embedded target):

```sh
rustup target add thumbv7em-none-eabihf
cd firmware && cargo check --target thumbv7em-none-eabihf
```

## ✅ Feature status

| Feature | Status |
|---|---|
| Native egui GUI | ✅ |
| PEAK PCAN-USB adapter | ✅ |
| KCAN Dongle adapter (STM32H753ZI) | ✅ |
| KCAN hardware timestamps (`hw_ts_us`) | ✅ |
| Adapter selection in GUI | ✅ |
| Dongle detection polling | ✅ |
| EDS optional per node | ✅ |
| Node ID from EDS `[DeviceComissioning]` | ✅ |
| NMT heartbeat / bootup monitoring | ✅ |
| NMT command sending (per-node + broadcast) | ✅ |
| SDO expedited upload / download | ✅ |
| SDO abort codes | ✅ |
| TPDO / RPDO live decode from EDS | ✅ |
| Raw PDO bytes for nodes without EDS | ✅ |
| Multi-node EDS mapping | ✅ |
| JSONL logging (received + sent) | ✅ |
| SDO segmented transfers | ✅ |
| SDO block transfers | ✅ |
| KCAN Phase 3: STM32H563 HW encryption | planned |
| CAN FD (KCAN firmware + host) | planned |
| EMCY message decode | planned |
| Heartbeat timeout / watchdog | planned |
| Replay from saved JSONL log | planned |

## Documentation

Additional documentation is available in the [`.readme/`](.readme/) folder:

- **[Multi-Node Stability Guide](.readme/multi-node-stability.md)** — Fixes for connection issues with 7-10+ nodes, error recovery, and troubleshooting
- **[Logging Performance](.readme/logging-performance.md)** — Optimization details for high-traffic scenarios, batched flushing, and configuration tuning

## 🛠️ Development

### Updating the app icon

When editing icon images in `host/assets/RustyCAN.iconset/`, regenerate the `.icns` file and commit both:

```sh
cd host/assets
iconutil -c icns RustyCAN.iconset -o RustyCAN.icns
git add RustyCAN.iconset/ RustyCAN.icns
git commit -m "Update app icon"
```

**Why?** The `.icns` file is a versioned build artifact tracked in the repository. CI builds use the committed version to ensure deterministic, reproducible releases without modifying the working tree during builds (which would add a `-dirty` suffix to version strings from `git describe --dirty`).

The `.iconset` folder contains source PNG images at multiple resolutions (16×16 through 512×512, with @2x variants). macOS's `iconutil` combines these into the installable `.icns` format required by DMG/app bundles.

## License

Licensed under either of [Apache-2.0](LICENSE) or MIT at your option.
