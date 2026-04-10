# Features

## ✨ Complete Feature List

| Feature | Details |
|---|---|
| 🖥️ **Native GUI** | egui/eframe window — no terminal required |
| 🔌 **Adapter selection** | Choose PEAK PCAN-USB or KCAN Dongle from the Connect screen |
| 🔧 **KCAN Dongle** | STM32H753ZI Nucleo firmware (Embassy); custom 80-byte USB protocol with hardware timestamps |
| ⏱️ **Hardware timestamps** | KCAN frames carry µs-precision timestamps from FDCAN TIM2; logged as `hw_ts_us` in JSONL |
| 🔍 **Dongle detection** | Connect button enabled only when the selected adapter is found; re-checked every 2 s |
| 🔄 **Automatic adapter fallback** | If configured adapter unavailable, automatically tries other types (PEAK ↔ KCAN) with notice |
| 🛡️ **Error resilience** | Continues running through adapter I/O errors; useful for waiting on unpowered buses or temporary disconnections |
| 👂 **Listen-only mode** | Optional passive mode — no frames are ever transmitted; toggle at connect time |
| 💾 **Configuration persistence** | Form settings (port, baud, nodes, DBC files) saved to JSON and restored on next launch; missing files filtered out |
| 📄 **EDS optional** | Per-node EDS files are optional; PDO frames without EDS show raw byte values |
| 🆔 **Node ID from EDS** | Browsing to an EDS file auto-fills the Node ID from `[DeviceComissioning] NodeId` |
| 🌐 **Multi-node** | Configure any number of CANopen nodes; nodes are optional when using DBC-only monitoring |
| 💓 **NMT monitoring** | Live Bootup / Pre-Operational / Operational / Stopped state per node with age |
| 📡 **NMT commands** | Send Start / Stop / Enter Pre-Op / Reset Node / Reset Comm to any node or broadcast all |
| 📊 **PDO live values** | Decode TPDO/RPDO signals from EDS mappings; raw hex bytes when no EDS is loaded |
| 🚗 **DBC signal decoding** | Load a `.dbc` file to decode any message/signal on the bus; runs in dual-mode alongside CANopen; Intel & Motorola byte orders, VAL_ descriptions — [details](dbc-signal-decoding.md) |
| 🔎 **SDO decode** | Expedited upload/download with EDS name lookup; abort codes displayed |
| 📶 **Bus load bar** | 20-block colour-coded bar in the status strip: blue ≤30 %, yellow 30–70 %, red >70 % |
| 🎞️ **Frame rate** | Rolling fps counter (2 s window) shown alongside total frame count |
| 📝 **JSONL logging** | Every event (received and sent) appended to a newline-delimited JSON file |

## ✅ Feature Status

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
