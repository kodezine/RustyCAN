# GUI Guide

## Connect Screen

```
┌─ Header ─────────────────────────────────────────────────────────────────┐
│   RustyCAN              │  Detected  │                    [ Connect ]    │
│  v0.0.4-11-gec90d46     │            │                                   │
└──────────────────────────────────────────────────────────────────────────┘

┌─ Connection ─────────────────────────────────────────────────────────────┐
│  Adapter:  ○ PEAK PCAN-USB   ● KCAN Dongle ★                             │
│  KCAN:     [ KCAN Dongle v1.0.0 (SN: 00000001) ▼ ]                       │
│  Port:     [ 1 ]  (hidden for KCAN)                                      │
│  Baud:     [ 250000 ▼ ]  ● Dongle: Connected                             │
│  SDO timeout: [ 500 ]                                                    │
│  Log:      [ rustycan.jsonl              ] [Browse…]                     │
│  Mode:     ☐ Listen-only (passive)                                       │
│  Logging:  ☐ Also write plain-text .log file                             │
└──────────────────────────────────────────────────────────────────────────┘

┌─ CANopen Nodes ──────────────────────────────────────────────────────────┐
│  Node ID   EDS file path                                                 │
│  [ 1     ] [ /path/to/motor.eds    ] [Browse…] [✕]                       │
│  [ 2     ] [                       ] [Browse…] [✕]                       │
│                                      [+ Add node]                        │
└──────────────────────────────────────────────────────────────────────────┘

┌─ DBC Nodes ──────────────────────────────────────────────────────────────┐
│  DBC file path                                                           │
│  [ /path/to/vehicle.dbc        ] [Browse…] [✕]                           │
│                                      [+ Add DBC file]                    │
└──────────────────────────────────────────────────────────────────────────┘

ℹ️  [14:32:45] KCAN Dongle detected successfully
⚠️  [14:32:50] Node ID 5 is used more than once
🛑  [14:33:01] Invalid baud rate: "25x000"

┌─ Footer ─────────────────────────────────────────────────────────────────┐
│ Bus [░░░░░░░░░░░░░░░░░░░░] 0.0%  │  0.0 fps  │        Total:           0 │
│ No log file                                                              │
└──────────────────────────────────────────────────────────────────────────┘
```

### Header Toolbar

**Header toolbar** — top bar present on all screens:
- **Logo and title** — RustyCAN logo (48×48px, rounded corners) with app name and git version
  - Version format: `v0.0.4-11-gec90d46` (tag + commits since tag + commit hash)
  - Falls back to Cargo.toml version when git is unavailable
- **Dongle status indicator** — shows connection state with icon and text:
  - 🟢 Green plug + "Detected" when adapter is found
  - 🔴 Red plug + "Not detected" when adapter is missing
  - Polled every 2 seconds in background thread
- **Connect button** — green button with plug icon, disabled until dongle detected
  - Tooltip explains why disabled ("Connect a CAN dongle first" or "Fix duplicate node IDs")

### Footer Status Bar

**Footer status bar** — bottom bar showing system state (greyed out on Connect screen):
- **Bus load bar** — 20 block characters showing estimated CAN bus utilization:
  - Empty blocks (░) in grey when not connected
  - Format: `Bus [░░░░░░░░░░░░░░░░░░░░] 0.0%`
- **FPS counter** — shows `📊 0.0 fps` (greyed out until connected)
- **Total frame count** — cumulative frames since connection: `Total: 0` (right-aligned)
- **Log file path** — shows `📄 No log file` until session starts

### Adapter Selection

**Adapter** — radio buttons to choose between PEAK PCAN-USB and KCAN Dongle.
The KCAN row (★) is the recommended first-class option.

**KCAN device** — dropdown listing all KCAN dongles found via USB enumeration
(VID `0x1209` / PID `0xBEEF`). Re-enumerated every 2 s.

**Port** — PCAN-USB channel number (typically `1`); hidden when KCAN is selected.

**Baud rate** — drop-down: 125000 / 250000 / 500000 / 1000000 bps.

**SDO timeout** — timeout in milliseconds for SDO transactions; defaults to 500ms.

**Log file** — base name for the `.jsonl` output; defaults to `rustycan.jsonl`.
A timestamp (`YYYYMMDDHHMMSS`) is automatically inserted before the extension,
so `log.jsonl` creates `log_20263003130458.jsonl`. This ensures each run
produces a unique log file.

**Mode** — "Listen-only (passive)" checkbox enables passive mode where no frames
are transmitted. Useful for monitoring without affecting the bus.

**Logging** — "Also write plain-text .log file" checkbox enables human-readable
log file alongside JSONL.

**Dongle indicator** — polled every 2 s; the Connect button stays disabled
(greyed) until the adapter is found on the given port/baud.

**Automatic adapter fallback** — if the configured adapter is not found during
probing, RustyCAN automatically tries other available adapter types (PEAK ↔ KCAN).
When a fallback succeeds, a blue notice appears: "⚠ PEAK PCAN-USB not found,
automatically switched to KCAN Dongle". Manually switching adapters clears the notice.

### CANopen Nodes Section

**CANopen Nodes** — separate collapsing header below Connection; holds CANopen nodes to monitor (optional):
- *Node ID* — decimal (`5`) or hex with `0x`/`H` prefix/suffix
  (`0x05`, `05H`); valid range 1–127.
- *EDS file* — optional; click **Browse…** to pick a file. If the EDS
  contains `[DeviceComissioning] NodeId`, the Node ID box is pre-filled
  automatically. Leave the EDS blank to monitor the node without decoding.
- **[+ Add node]** button adds a new node row.
- **[✕]** button removes a node (with confirmation prompt).
- CANopen nodes are entirely optional — you can connect with zero nodes
  when using DBC signal decoding or raw frame logging only.

### DBC Nodes Section

**DBC Nodes** — separate collapsing header below CANopen Nodes; holds DBC files for signal decoding (optional, see [DBC details](dbc-signal-decoding.md)):
- Collapsed by default; click to expand/collapse.
- *DBC file path* — path to `.dbc` file; click **Browse…** to select.
- Load multiple DBC files; first-match precedence for overlapping CAN IDs.
- **[+ Add DBC file]** button adds a new DBC file row.
- **[✕]** button removes a DBC file (with confirmation prompt).
- Runs in dual-mode alongside CANopen; frames can be decoded by both systems.
- DBC files are entirely optional; useful for vehicle protocols that don't use CANopen.

### Message History

**Message History** — displays the last 5 errors, warnings, and notices with persistent history:
- **Timestamps** — each message shows the wall-clock time when it occurred in `[HH:MM:SS]` format (UTC)
- **Icons** — distinct icons identify message types:
  - 🛑 **ERROR** (red exclamation-circle) — connection failures, invalid configuration
  - ⚠️  **WARN** (yellow exclamation-triangle) — duplicate node IDs, missing files
  - ℹ️  **INFO** (blue info-circle) — adapter auto-switch notifications, successful detections
- **Fade-out effect** — messages gradually fade from their original color to grey over 5-10 seconds:
  - 0-5 seconds: full color (red/yellow/blue)
  - 5-10 seconds: linear interpolation to grey
  - After 10 seconds: fully grey but remains visible
- **Persistence** — messages never disappear; the last 5 are always visible for reference
- **Font size** — consistent 11pt for both timestamps and message text
- **Common messages**:
  - Adapter fallback: "PEAK PCAN-USB not found, automatically switched to KCAN Dongle"
  - Invalid input: "Invalid baud rate: \"25x000\""
  - Validation errors: "Node ID 5 is used more than once"

Messages appear centered above the Connect button, providing at-a-glance status feedback
without requiring active monitoring.

### Connect Button

The **Connect** button appears below the message history. It remains greyed out (disabled)
until the selected adapter is detected during the automatic 2-second polling cycle.
Once the adapter is found, the button becomes active and clicking it transitions
to the Monitor screen.

**Configuration persistence** — all settings are saved to
`~/Library/Application Support/RustyCAN/config.json` (macOS) when you
successfully connect, and automatically restored on next launch. Files that
no longer exist are silently removed from the restored configuration.

## Monitor Screen

**Error resilience** — RustyCAN continues running even when the adapter
encounters I/O errors (e.g., USB timeouts, temporary disconnections). This
allows you to:
- Connect to the adapter before the CAN bus is powered
- Wait for nodes to come online without manual reconnection
- Tolerate temporary USB communication issues

Errors are logged to the terminal (stderr) for debugging. Use the **Disconnect**
button to manually return to the Connect screen when needed.

```
┌─ Header ────────────────────────────────────────────────────────────────────────────┐
│  RustyCAN               │  🔌 Port 1  ·  📡 250,000 bps  ·  👥 2 node(s)          │
│  v0.0.4-11-gec90d46     │  🟡 LISTEN-ONLY              [ ⚠️  Disconnect ]        │
└─────────────────────────────────────────────────────────────────────────────────────┘

┌─ NMT Status ───────────────────────────────────────────────────────────────┐
│ Broadcast: [Start] [Stop] [Pre-Op] [Reset] [Reset Comm]                    │
│ Node  EDS            State           Last seen  Actions                    │
│    1  motor.eds      OPERATIONAL     0.3s ago   [Start][Stop]…             │
│    2  (no EDS)       PRE-OPERATIONAL 1.1s ago   [Start][Stop]…             │
├─ PDO Live Values ──────────────────────────────────────────────────────────┤
│ Node  PDO  Signal              Value       Updated                         │
│    1    1  StatusWord          39          0.05s ago                       │
│    1    1  VelocityActualValue 1234        0.05s ago                       │
│    2    1  Byte0               [AB]        0.12s ago                       │
│    2    1  Byte1               [CD]        0.12s ago                       │
├─ DBC Signals ──────────────────────────────────────────────────────────────┤
│ Message            Signal           Value    Unit   Age        Count       │
│ 0x123 VehicleSpeed Speed            65       km/h   0.15s ago  142         │
│ 0x200 EngineData   RPM              2500     rpm    0.08s ago  89          │
├─ SDO Browser ──────────────────────────────────────────────────────────────┤
│ [Node: 1 (motor.eds) ▼]  [Index: 0x6040 ▼]  [Read] [Write]                 │
│ 0x6040/00  ControlWord     15 [0x0F]                                       │
├─ SDO Log (last 50) ────────────────────────────────────────────────────────┤
│ [12:01:01.234] N01 READ  6040h/00 ControlWord = 15                         │
│ [12:01:01.456] N01 WRITE 6040h/00 ControlWord = 15                         │
└────────────────────────────────────────────────────────────────────────────┘

┌─ Footer ───────────────────────────────────────────────────────────────────┐
│ Bus [██████░░░░░░░░░░░░░░] 28.4%  │  📊 234.0 fps  │  Total:        45,231 │
│ 📄 rustycan_20260410143045.jsonl                                           │
└────────────────────────────────────────────────────────────────────────────┘
```

### Header Toolbar

**Header toolbar** — connection info and controls:
- **Logo and title** — RustyCAN logo with app name and git version (same as Connect screen)
- **Connection details** — adapter info displayed with icons:
  - 🔌 Port number (e.g., "Port 1") — only shown for PEAK adapter
  - 📡 Baud rate with thousands separator (e.g., "250,000 bps")
  - 👥 Node count (e.g., "2 node(s)")
- **Listen-only indicator** — yellow 🟡 badge "LISTEN-ONLY" shown when passive mode is active
- **Disconnect button** — red button with warning icon (⚠️) to return to Connect screen
  - Tooltip: "Return to configuration screen"
  - Preserves form state (no data loss)
- **Globe icon (🌐)** — hyperlink that opens the live browser dashboard (`http://localhost:7878/`) in a new browser tab; blue while monitoring
- **Chart icon (📈)** — toggles the native plot window; grey when closed, blue when open

### Footer Status Bar

**Footer status bar** — active monitoring display:
- **Bus load bar** — 20 block characters with color-coded zones:
  - Blue (██) for 0-30% utilization
  - Yellow (██) for 30-70% utilization  
  - Red (██) for >70% utilization
  - Percentage label matches the color of the highest filled zone
  - Hover tooltip shows formula: `fps × 125 bits ÷ baud_rate × 100`
- **FPS counter** — rolling average over 2-second window
  - Format: `📊 234.0 fps`
- **Total frame count** — cumulative frames since connection started
  - Format: `Total: 45,231` (comma-separated, right-aligned)
  - Supports up to 999,999,999,999 (trillion) frames
- **Log file path** — JSONL filename with hover tooltip showing full path
  - Format: `📄 rustycan_20260410143045.jsonl`
  - Timestamp in filename prevents overwriting previous logs
  - Smart truncation keeps filename visible with ellipses in middle if path is too long

### NMT Status

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

### PDO Live Values

**PDO Live Values** — signals decoded from EDS mappings. For nodes without an
EDS the raw frame bytes appear as `Byte0`, `Byte1`, … in hex.

### DBC Signals

**DBC Signals** — collapsing header showing decoded signals from loaded DBC files:
- *Message* — CAN ID and message name from DBC (e.g., `0x123 VehicleSpeed`)
- *Signal* — signal name from DBC definition
- *Value* — physical value (scaled/offset applied); VAL_ descriptions shown in parentheses
- *Unit* — engineering unit from DBC
- *Age* — time since last update
- *Count* — number of times this signal has been received
- Signals sorted by CAN ID then signal name
- Shows "(no DBC loaded)" message when no DBC files are configured
- Runs in parallel with CANopen decoding; same frame can appear in both sections

### SDO Browser

**SDO Browser** — collapsing header for interactive SDO read/write operations:
- *Node selector* — dropdown to choose which configured node to query
- *Object dictionary browser* — table showing all objects from the node's EDS
- *Read/Write buttons* — perform SDO upload/download on selected object
- Values displayed in both decimal and hex format (e.g., `42 [0x2A]`)
- String values show readable text followed by hex bytes
- **Disabled in listen-only mode** to prevent accidental bus writes
- Click any row to select it; entire row (all columns) is clickable

**SDO Log** — scrollable ring buffer (last 50 entries). `READ` entries are
coloured cyan; `WRITE` entries magenta. Abort codes appear in red. Values are
displayed with both their primary format and hex/ASCII representation.

### Status Bar

**Header toolbar** — connection info with listen-only indicator:
- Adapter info (port/baud rate)
- Node count
- Yellow "LISTEN-ONLY" badge when passive mode is active
- Disconnect button (top-right)

**Status bar** — three items separated by dividers:
- **fps + total** — rolling frames/second (2 s window) and cumulative frame count.
- **Bus load bar** — 20 block characters (`█`/`░`) colour-coded by zone: blue for
  ≤ 30 %, yellow for 30–70 %, red for > 70 %. The percentage after the bar matches
  the colour of the highest zone reached. Load is estimated as
  `fps × 125 bits ÷ baud_rate × 100` (standard 11-bit ID, 8-byte frame with
  overhead/stuffing). Hover the bar to see the formula.
- **Log path** — filename of the active `.jsonl` log with file icon; hover to see the full path.
