# DBC Signal Decoding

RustyCAN can decode raw CAN frames into named, scaled signals using an
industry-standard **DBC (CAN database)** file.  DBC decoding runs in parallel
with CANopen — you can watch PDO live values and DBC signals at the same time,
from the same adapter, without any loss of frame data.

---

## What is a DBC file?

A DBC file describes every message on a CAN bus:

```
BO_ 770 EngineData: 8 Vector__XXX
 SG_ EngineSpeed  : 0|16@1+ (0.125,0) [0|8031.875] "rpm"  Vector__XXX
 SG_ CoolantTemp  : 16|8@1+ (1,-40)   [-40|215]    "degC" Vector__XXX
 SG_ ThrottlePos  : 24|8@0+ (0.392157,0) [0|100]   "%"    Vector__XXX

VAL_ 770 CoolantTemp 0 "Sensor_Error" ;
```

Each signal definition contains:

| Field | Meaning |
|---|---|
| `start_bit` | LSBit position (Intel) or MSBit position (Motorola) |
| `length` | Number of bits |
| `@1` / `@0` | Byte order: **Intel (little-endian)** / **Motorola (big-endian)** |
| `+` / `-` | Value type: unsigned / signed (two's complement) |
| `(factor, offset)` | `physical = raw × factor + offset` |
| `[min|max]` | Physical value range (informational) |
| `"unit"` | Engineering unit string |

`VAL_` entries map raw integer values to human-readable descriptions (e.g. error
codes, enumerated states).  These appear as tooltips in the DBC Signals panel.

---

## Loading DBC files

1. Open RustyCAN, click **Connect**.
2. Expand the **DBC Nodes** section.
3. Click the **+** button to add a new DBC file entry.
4. Click the folder icon to **Browse** for your `.dbc` file, or type the path directly.
   - The filename is shown when the field is idle; hover for the full path.
   - Click the trash icon to remove a DBC file entry (with confirmation).
5. Repeat steps 3-4 to add additional DBC files (unlimited).
6. Fill in adapter, baud rate, and CANopen node settings as usual, then click
   **Connect**.

All DBC files are validated when the session starts. If any file fails to parse
(e.g. syntax error), the connect attempt is rejected with a clear error message
before any adapter is opened.

**Encoding support** — DBC files are read as UTF-8 by default.  If the file
contains characters outside of ASCII (common for files produced by Vector
CANdb++ or PEAK), RustyCAN automatically falls back to the **CP-1252**
(Windows-1252) encoding.

**Multi-DBC merging** — When multiple DBC files define the same CAN ID, all
definitions are preserved and the first-loaded file takes precedence during
decoding. The `source_dbc` field in decoded signals and JSONL logs identifies
which file provided the decoded data.

---
## Monitor panel

Once connected, the **DBC Signals** collapsible panel appears in the Monitor
screen, below the PDO Live Values panel.

### Three display states

| State | What you see |
|---|---|
| No DBC loaded | *(no DBC loaded — browse for a .dbc file on the Connect screen)* |
| DBC loaded, no matching frames yet | *DBC: EngineData — waiting for matching frames…* |
| Matching frames received | Live signal table (see below) |

### Signal table columns

| Column | Contents |
|---|---|
| **Message** | CAN ID (hex) and DBC message name |
| **Signal** | Signal name from the DBC |
| **Value** | Decoded physical value (`raw × factor + offset`) |
| **Unit** | Engineering unit string |
| **Age** | Time since the last frame containing this signal |
| **Count** | Total number of frames decoded for this signal |

When a `VAL_` description maps to the current raw value, the value cell shows
`<physical> (<description>)` in a highlighted colour.  Hovering over that cell
reveals the underlying raw integer.

---

## Supported signal encodings

| Encoding | Support |
|---|---|
| **Intel (little-endian)** bit order | ✅ Full |
| **Motorola (big-endian)** bit order | ✅ Full |
| **Unsigned** integers | ✅ |
| **Signed** integers (two's complement) | ✅ |
| **VAL_** value descriptions | ✅ |
| Multiplexed signals (`M` / `m<N>`) | ⚠️ Multiplexor and Plain signals are decoded; *multiplexed* signals (those selected by a switch value) are skipped in the current release |
| Extended (29-bit) CAN IDs | ✅ |

---

## Dual-mode operation

DBC decoding is completely independent of CANopen.  Every frame is always
processed by both decoders:

```
CAN frame received
    │
    ├─► CANopen classify_frame()  ──► NMT / PDO / SDO handling
    │
    └─► DBC decode_frame()        ──► DBC Signals panel
```

This means you can, for example, run CANopen NMT monitoring for a device while
simultaneously decoding manufacturer-specific messages defined only in a DBC.

---

## JSONL logging format for decoded DBC signals

### Implementation status

Decoded DBC signals are now **written to JSONL** as `DBC_SIGNAL` entries and
displayed in the **DBC Signals** monitor panel. Multiple DBC files can be loaded
simultaneously; they are merged with first-match precedence for overlapping CAN IDs.

### JSONL contract (`DBC_SIGNAL`)

Each decoded frame emits one JSONL line with this shape:

| Field | Type | Description |
|---|---|---|
| `ts` | string | ISO 8601 timestamp (UTC, millisecond precision) |
| `type` | string | Always `"DBC_SIGNAL"` |
| `cob_id` | string | CAN ID as `"0xNNN"` or `"0x1FFFFFFF"` |
| `source_node` | number/null | Node inferred from CANopen ranges when possible; otherwise `null` |
| `message` | string | DBC message name (`BO_` name) |
| `signals` | object | Map of signal name to decoded signal object |
| `raw` | string[] | Original frame bytes as `"0x##"` strings |
| `hw_ts_us` | number (optional) | KCAN hardware timestamp in microseconds |

Each `signals[<name>]` object is expected to contain:

| Field | Type | Description |
|---|---|---|
| `raw` | number | Raw integer before scaling |
| `physical` | number | Scaled value (`raw * factor + offset`) |
| `unit` | string | Unit from DBC (`"rpm"`, `"degC"`, `%`, etc.) |
| `description` | string/null | `VAL_` description for current raw value, if defined |

Example line:

```jsonl
{"ts":"2026-04-10T09:14:22.481Z","type":"DBC_SIGNAL","cob_id":"0x302","source_node":null,"message":"EngineData","signals":{"EngineSpeed":{"raw":800,"physical":100.0,"unit":"rpm","description":null},"CoolantTemp":{"raw":0,"physical":-40.0,"unit":"degC","description":"Sensor_Error"},"ThrottlePos":{"raw":0,"physical":0.0,"unit":"%","description":null}},"raw":["0x20","0x03","0x00","0x00","0x00","0x00","0x00","0x00"],"hw_ts_us":1835042}
```

This contract mirrors the runtime decoder output used by the DBC panel:
message-level metadata plus per-signal raw/physical/unit/description values.

---

## Technical notes

### Bit extraction

**Intel (little-endian):** `start_bit` is the LSBit position in a flat
numbering where bit 0 is the LSB of byte 0.  Bits are extracted LSB-first
(ascending bit positions).

**Motorola (big-endian):** `start_bit` is the MSBit position in the same flat
numbering (bit 0 = LSB of byte 0, bit 7 = MSB of byte 0).  Bits are extracted
MSBit-first, decrementing within a byte and jumping to the MSB of the next
byte when a byte boundary is crossed.

### ID normalisation

DBC Standard IDs are stored as `u32` values equal to the 11-bit identifier.
DBC Extended IDs are stored as the 29-bit value (the EFF marker bit is
stripped).  The same normalisation is applied to the received `embedded_can`
frame ID before the lookup.

### Crate used

Signal parsing is provided by the [`can-dbc`](https://crates.io/crates/can-dbc)
crate (v9).

---

## Example DBC (matches the integration test fixture)

```dbc
VERSION ""

NS_ :

BS_:

BU_: Vector__XXX ECU1

BO_ 770 EngineData: 8 Vector__XXX
 SG_ EngineSpeed : 0|16@1+ (0.125,0) [0|8031.875] "rpm" Vector__XXX
 SG_ CoolantTemp : 16|8@1+ (1,-40) [-40|215] "degC" Vector__XXX
 SG_ ThrottlePos : 24|8@0+ (0.392157,0) [0|100] "%" Vector__XXX

VAL_ 770 CoolantTemp 0 "Sensor_Error" ;
```

With a CAN frame `0x302` (decimal 770) and payload `[0x20, 0x03, 0x00, …]`:

| Signal | Raw | Physical | Unit |
|---|---|---|---|
| EngineSpeed | 800 | **100.0** | rpm |
| CoolantTemp | 0 | **−40.0** | degC |
| ThrottlePos | 0 | **0.0** | % |

`CoolantTemp` raw value 0 also resolves the `VAL_` description `"Sensor_Error"`.
