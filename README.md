# RustyCAN

CANopen viewer for macOS — log and analyze SDO, PDO, and NMT events from
multiple devices described by EDS files.

## Features

- **Multi-device** — map any number of EDS files to Node-IDs at startup
- **NMT tracking** — live Bootup / Pre-Operational / Operational / Stopped state per node
- **SDO decode** — expedited upload and download transactions with name lookup from EDS
- **PDO live values** — decode all signals from TPDO/RPDO frames using EDS object mappings
- **Terminal TUI** — ratatui split-panel interface with NMT status, PDO live values, SDO log
- **JSONL logging** — every decoded event is appended to a newline-delimited JSON file

## Prerequisites

### Hardware

A **PEAK PCAN-USB** adapter is required on macOS.

### PCUSB library

Install the PCANBasic userspace library from **<https://mac-can.com>**:

1. Download the latest *PCUSB* `.pkg` for macOS from the mac-can.com downloads page.
2. Run the installer — it places `libPCBUSB.dylib` in `/usr/local/lib/`.
3. Connect your PCAN-USB adapter; it will appear as channel `1`.

### Rust toolchain

```sh
rustup update stable  # MSRV: 1.85+
```

## Build

```sh
git clone https://github.com/kodezine/RustyCAN
cd RustyCAN
cargo build --release
```

## Run

```sh
# One device — node 1 described by motor.eds, 250 kbps
cargo run --release -- --port 1 --baud 250000 --node 1:motor.eds

# Multiple devices
cargo run --release -- \
    --port 1 --baud 250000 \
    --node 1:motor.eds \
    --node 2:sensor.eds \
    --node 3:io_module.eds \
    --log session.jsonl
```

| Flag | Default | Description |
|------|---------|-------------|
| `--port` | `1` | PCAN-USB channel (1–8) |
| `--baud` | `250000` | CAN baud rate in bps (e.g. 125000, 250000, 500000, 1000000) |
| `--node ID:PATH` | _(required)_ | Map Node-ID to EDS file; repeat for each device |
| `--log` | `rustycan.jsonl` | Output JSONL log file |

Press **`q`** or **Ctrl-C** to exit.

## TUI layout

```
┌─ NMT Status ──────────────────────────────────────────────┐
│  Node  EDS               State            Last seen       │
│     1  motor.eds         OPERATIONAL      0.3s ago        │
│     2  sensor.eds        PRE-OPERATIONAL  1.1s ago        │
├─ PDO Live Values ─────────────────────────────────────────┤
│  Node  PDO  Signal             Value      Updated         │
│     1    1  StatusWord         0x0027     0.05s ago       │
│     1    1  VelocityActualValue 1234      0.05s ago       │
├─ SDO Log (last 50) ───────────────────────────────────────┤
│ [12:01:01.234] N01 READ  6040h/00 ControlWord = 0x000F    │
│ [12:01:01.456] N01 WRITE 6040h/00 ControlWord = 0x000F    │
└─ Frames/s:  234.0  Total:     45231  Log: session.jsonl ──┘
```

## JSONL log format

Each line is a self-contained JSON object:

```jsonl
{"ts":"2026-03-28T12:01:01.234Z","type":"NMT_STATE","node":1,"state":"OPERATIONAL"}
{"ts":"2026-03-28T12:01:01.235Z","type":"SDO_READ","node":1,"index":24640,"subindex":0,"name":"ControlWord","value":15,"raw":[75,64,96,0,15,0,0,0]}
{"ts":"2026-03-28T12:01:01.300Z","type":"PDO","node":1,"pdo_num":1,"signals":{"StatusWord":39,"VelocityActualValue":1234},"raw":[39,0,210,4,0,0,0,0]}
{"ts":"2026-03-28T12:01:01.400Z","type":"NMT_COMMAND","command":"START","target_node":0}
```

## Project structure

```
src/
  lib.rs                  public library surface
  main.rs                 CLI entry-point, CAN recv thread, wiring
  eds/
    mod.rs                EDS INI file parser
    types.rs              ObjectDictionary, DataType, AccessType
  canopen/
    mod.rs                COB-ID classification (classify_frame)
    nmt.rs                NMT command and heartbeat decoding
    sdo.rs                Expedited SDO decode (upload/download/abort)
    pdo.rs                PDO decoder built from EDS object mappings
  logger.rs               JSONL event logger (EventLogger)
  tui/
    mod.rs                ratatui terminal app loop, AppState
    widgets.rs            NMT, PDO, SDO, stats panel renderers
tests/
  integration_test.rs     End-to-end EDS + PDO + SDO + NMT tests
  fixtures/
    sample_drive.eds      CiA 402 servo drive fixture
```

## Testing

```sh
cargo test
```

34 tests (27 unit + 7 integration) covering the EDS parser, PDO bit extraction,
SDO command-specifier decode, and NMT state machine.

## Scope (v0.1)

| Feature | Status |
|---------|--------|
| NMT command decode | ✅ |
| NMT heartbeat / bootup decode | ✅ |
| SDO expedited upload / download | ✅ |
| SDO abort | ✅ |
| TPDO / RPDO live decode from EDS | ✅ |
| Multi-node EDS mapping | ✅ |
| ratatui TUI | ✅ |
| JSONL logging | ✅ |
| SDO segmented transfers | planned |
| EMCY messages | planned |
| Heartbeat monitoring / timeout | planned |
| CAN FD | planned |
| Replay from candump log | planned |

## License

Licensed under either of [Apache-2.0](LICENSE) or MIT at your option.
