# JSONL Log Format

Each line is a self-contained JSON object flushed immediately to disk.
All CAN data bytes are written as `"0x##"` hex strings.

## Example Log

```jsonl
{"ts":"2026-03-28T12:01:01.234Z","type":"NMT_STATE","cob_id":"0x720","node":32,"state":"PRE-OPERATIONAL","raw":["0x7F"]}
{"ts":"2026-03-28T12:01:01.235Z","type":"SDO_READ","cob_id":"0x5A0","node":32,"index":"0x3000","subindex":"0x01","name":"Status Word","value":255,"raw":["0x4B","0x00","0x30","0x01","0xFF","0x00","0x00","0x00"]}
{"ts":"2026-03-28T12:01:01.240Z","type":"SDO_READ","cob_id":"0x5A0","node":32,"index":"0x1008","subindex":"0x00","name":"Device Name","value":[84,67,45,77,78,50,48,56,54,52,55,52,45,48,48,0],"ascii":"TC-MN2086474-00","raw":["0x43","0x08","0x10","0x00","0x54","0x43","0x2D","0x4D"]}
{"ts":"2026-03-28T12:01:01.300Z","type":"PDO","cob_id":"0x201","node":32,"pdo_num":1,"signals":{"Status Word":43,"Digital Inputs":0,"Current Segment Index":0},"raw":["0x2B","0x00","0x00","0x00"]}
{"ts":"2026-03-28T12:01:01.400Z","type":"NMT_COMMAND","cob_id":"0x000","command":"START","target_node":0,"raw":["0x01","0x00"]}
{"ts":"2026-03-28T12:01:01.401Z","type":"NMT_COMMAND_SENT","cob_id":"0x000","command":"START","target_node":1,"raw":["0x01","0x01"]}
```

## Common Fields

| Field | Present in | Description |
|---|---|---|
| `ts` | all | ISO 8601 timestamp with millisecond precision |
| `type` | all | Entry type (see table below) |
| `cob_id` | all | CAN Object Identifier as `"0xNNN"` hex string |
| `raw` | all | Full CAN data bytes as `["0x##", …]` hex strings |
| `hw_ts_us` | KCAN only | Hardware timestamp in microseconds from FDCAN TIM2 (absent for PEAK frames) |

## Entry Types

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

## Notes

**DBC note:** For decoded DBC signal JSONL contract details, see
[`dbc-signal-decoding.md`](dbc-signal-decoding.md#jsonl-logging-format-for-decoded-dbc-signals).
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
