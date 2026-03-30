# RustyCAN

A native macOS GUI for monitoring, decoding, and controlling CANopen networks.
Connect a **PEAK PCAN-USB** adapter, optionally provide EDS device-description
files, and get live NMT state, PDO signal values, and SDO transactions — all
stored to a newline-delimited JSON log.

## Features

| Feature | Details |
|---|---|
| **Native GUI** | egui/eframe window — no terminal required |
| **Dongle detection** | Connect button enabled only when a PCAN-USB adapter is found; re-checked every 2 s |
| **Listen-only mode** | Optional passive mode — no frames are ever transmitted; toggle at connect time |
| **EDS optional** | Per-node EDS files are optional; PDO frames without EDS show raw byte values |
| **Node ID from EDS** | Browsing to an EDS file auto-fills the Node ID from `[DeviceComissioning] NodeId` |
| **Multi-node** | Configure any number of nodes at startup; new nodes appear dynamically from heartbeats |
| **NMT monitoring** | Live Bootup / Pre-Operational / Operational / Stopped state per node with age |
| **NMT commands** | Send Start / Stop / Enter Pre-Op / Reset Node / Reset Comm to any node or broadcast all |
| **PDO live values** | Decode TPDO/RPDO signals from EDS mappings; raw hex bytes when no EDS is loaded |
| **SDO decode** | Expedited upload/download with EDS name lookup; abort codes displayed |
| **Bus load bar** | 20-block colour-coded bar in the status strip: blue ≤30 %, yellow 30–70 %, red >70 % |
| **Frame rate** | Rolling fps counter (2 s window) shown alongside total frame count |
| **JSONL logging** | Every event (received and sent) appended to a newline-delimited JSON file |

## Prerequisites

### Hardware

A **PEAK PCAN-USB** adapter is required. Other adapters are not supported yet.

### PCUSB driver library

Install the PCANBasic userspace library from **<https://mac-can.com>**:

1. Download the latest *PCUSB* `.pkg` from the mac-can.com downloads page.
2. Run the installer — it places `libPCBUSB.dylib` in `/usr/local/lib/`.
3. Connect your PCAN-USB adapter; it appears as channel `1` by default.

### Rust toolchain

```sh
rustup update stable  # MSRV: 1.85+
```

## Build & run

```sh
git clone https://github.com/kodezine/RustyCAN
cd RustyCAN
cargo run --release
```

The GUI window opens immediately. No command-line flags are required.

## Using the GUI

### Connect screen

```
┌─ Connection ──────────────────────────────────────────────┐
│  Port:  [ 1 ]  ● Dongle: Connected                        │
│  Baud:  [ 250000 ▼ ]                                      │
│  Log:   [ rustycan.jsonl              ] [Browse…]         │
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

**Port** — PCAN-USB channel number (typically `1`).  
**Baud rate** — drop-down: 125000 / 250000 / 500000 / 1000000 bps.  
**Log file** — base name for the `.jsonl` output; defaults to `rustycan.jsonl`.
A timestamp (`YYYYMMDDHHMMSS`) is automatically inserted before the extension,
so `log.jsonl` creates `log_20263003130458.jsonl`. This ensures each run
produces a unique log file.  
**Dongle indicator** — polled every 2 s; the Connect button stays disabled
(greyed) until the adapter is found on the given port/baud.  
**Nodes** — each row is a CANopen node:
- *Node ID* — decimal (`5`) or hex with `0x`/`H` prefix/suffix
  (`0x05`, `05H`); valid range 1–127.
- *EDS file* — optional; click **Browse…** to pick a file. If the EDS
  contains `[DeviceComissioning] NodeId`, the Node ID box is pre-filled
  automatically. Leave the EDS blank to monitor the node without decoding.
- Zero nodes configured is valid — the monitor will still show any node
  that sends a heartbeat frame.

### Monitor screen

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

## JSONL log format

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

### Entry types

| `type` | Trigger | Key fields |
|---|---|---|
| `NMT_STATE` | Heartbeat or bootup frame received | `node`, `state` |
| `NMT_COMMAND` | NMT command frame observed on bus | `command`, `target_node` |
| `NMT_COMMAND_SENT` | NMT command sent by RustyCAN | `command`, `target_node` |
| `SDO_READ` | SDO upload response decoded | `node`, `index` (hex), `subindex` (hex), `name`, `value`, `ascii` (optional) |
| `SDO_WRITE` | SDO download request decoded | `node`, `index` (hex), `subindex` (hex), `name`, `value`, `ascii` (optional) |
| `PDO` | TPDO or RPDO frame decoded | `node`, `pdo_num` (EDS-derived), `signals` (name→value map) |

**SDO notes:**
- `ascii` — present only for byte array values (VISIBLE_STRING, OCTET_STRING) when
  all bytes are printable ASCII (0x20–0x7F), common whitespace characters (tab,
  newline, carriage return), or null terminators. Trailing nulls are stripped
  from the `ascii` string. Makes it easy to read string values directly in the
  log without manual decoding.

**PDO notes:**
- `node` and `pdo_num` are resolved from the EDS mapping for the matching COB-ID; if no EDS is loaded for the sending node, `node` is derived from the COB-ID range and signals fall back to `{"Byte0": "0x##", …}`.
- `signals` preserves EDS declaration order; typed values match the EDS `DataType` (integer, unsigned, float, or string).

## Project structure

```
src/
  lib.rs          public library surface
  main.rs         binary entry-point (launches GUI)
  app.rs          AppState, CanEvent enum, event application logic
  session.rs      SessionConfig, CanCommand, adapter lifecycle, recv thread
  logger.rs       EventLogger — JSONL line writer
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
```

## Testing

```sh
cargo test
```

36 tests (29 unit + 7 integration) covering the EDS parser, node-ID string
parsing, PDO bit extraction, SDO command-specifier decode, NMT encode/decode,
and the COB-ID classifier.

## Feature status

| Feature | Status |
|---|---|
| Native egui GUI | ✅ |
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
| SDO block transfers | planned |
| EMCY message decode | planned |
| Heartbeat timeout / watchdog | planned |
| CAN FD support | planned |
| Replay from saved JSONL log | planned |

## Documentation

Additional documentation is available in the [`.readme/`](.readme/) folder:

- **[Multi-Node Stability Guide](.readme/multi-node-stability.md)** — Fixes for connection issues with 7-10+ nodes, error recovery, and troubleshooting
- **[Logging Performance](.readme/logging-performance.md)** — Optimization details for high-traffic scenarios, batched flushing, and configuration tuning

## License

Licensed under either of [Apache-2.0](LICENSE) or MIT at your option.
