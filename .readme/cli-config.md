# CLI Configuration File

RustyCAN can be launched with a JSON configuration file so it connects to the
CAN bus automatically — no interaction with the connect form required.

```sh
rustycan --config myconfig.json
```

The window still opens normally; the connect form is simply skipped.  If the
configuration file describes a valid session, the monitor view appears
immediately.  If something is wrong the connect form is shown with an error
message so you can correct it interactively.

## Flags

| Flag | Value | Description |
|---|---|---|
| `--config <FILE>` | path to a `.json` file | Load settings and auto-connect on launch |
| `--http-port <PORT>` | `1024`–`65535` | Override the live dashboard port (see [Precedence](#port-precedence)) |

```sh
# Connect from a config file, dashboard on port 9090
rustycan --config myconfig.json --http-port 9090

# Just change the dashboard port, use interactive connect form
rustycan --http-port 9090

# Print all available options
rustycan --help
```

## JSON Schema

A minimal valid configuration requires only `adapter_kind`, `port`, and
`baud`.  All other fields are optional and fall back to the defaults shown
below.

```json
{
  "adapter_kind": "Peak",
  "port": "1",
  "baud": "250000",
  "http_port": 7878,
  "log_path": "rustycan.jsonl",
  "sdo_timeout_str": "500",
  "listen_only": false,
  "text_log": false,
  "kcan_serial": "",
  "nodes": [
    { "id_str": "1", "eds_path": "/path/to/node1.eds" },
    { "id_str": "2", "eds_path": "" }
  ],
  "dbc_files": [
    { "path": "/path/to/bus.dbc" }
  ]
}
```

A fully annotated copy is included in the repository at
[`host/config.example.json`](../host/config.example.json).

### Field Reference

| Field | Type | Default | Description |
|---|---|---|---|
| `adapter_kind` | `"Peak"` \| `{"KCan":{"serial":null}}` | — | Which hardware adapter to use |
| `port` | string | — | Adapter channel (PEAK: `"1"` = PCAN_USBBUS1; unused for KCAN) |
| `baud` | string | — | CAN baud rate in bps, e.g. `"250000"` or `"500000"` |
| `http_port` | integer | `7878` | Port for `http://127.0.0.1:<port>/` live dashboard |
| `log_path` | string | `"rustycan.jsonl"` | Path for the JSONL log file; relative paths are resolved from the working directory |
| `sdo_timeout_str` | string | `"500"` | SDO response timeout in milliseconds |
| `listen_only` | bool | `false` | When `true`, no CAN frames are transmitted |
| `text_log` | bool | `false` | When `true`, also write a plain-text `.log` alongside the JSONL file |
| `kcan_serial` | string | `""` | Pin a specific KCAN dongle by USB serial; empty = first found |
| `nodes` | array | `[]` | CANopen nodes to monitor; `eds_path` may be empty |
| `dbc_files` | array | `[]` | DBC files for raw bus signal decoding |

#### `adapter_kind` values

PEAK PCAN-USB:
```json
"adapter_kind": "Peak"
```

KCAN Dongle (auto-select first found):
```json
"adapter_kind": { "KCan": { "serial": null } }
```

KCAN Dongle pinned to a specific serial:
```json
"adapter_kind": { "KCan": { "serial": "ABC123" } }
```

#### `nodes` array

Each entry has two fields:

| Field | Description |
|---|---|
| `id_str` | Node ID as decimal (`"1"`) or hex (`"0x01"`, `"01H"`) |
| `eds_path` | Absolute path to an EDS file, or `""` for raw monitoring only |

Non-existent EDS files are silently removed at load time so the session can
still start.

#### `dbc_files` array

Each entry has a single `path` field pointing to a `.dbc` file.  Non-existent
paths are silently removed at load time.

## Port Precedence

When the dashboard port is specified in more than one place the following
priority applies (highest first):

1. `--http-port` CLI flag
2. `"http_port"` field in the JSON config file
3. Built-in default: **7878**

## Auto-Connect Behaviour

When `--config` is supplied:

* **Valid file, valid settings** → connect form is skipped; monitor view opens.
* **Valid file, connect error** (e.g. adapter not found) → connect form opens
  with the loaded settings pre-filled and an error banner at the top.  You can
  fix the issue and click Connect.
* **File not found or invalid JSON** → connect form opens with default settings
  and an error banner describing the problem.

Without `--config` the application behaves exactly as before: the connect form
opens using settings from the previous session (stored in the platform
app-data directory) or factory defaults.

## Configuration File Location (GUI-saved)

When you click **Connect** in the GUI the current form state is saved
automatically to a platform-specific location.  This file is independent of
any `--config` file you supply on the command line.

| Platform | Path |
|---|---|
| macOS | `~/Library/Application Support/RustyCAN/config.json` |
| Linux | `~/.local/share/RustyCAN/config.json` |
| Windows | `%APPDATA%\RustyCAN\config.json` |

The GUI-saved file uses the same JSON schema and can be copied, shared, or
passed back to `--config` directly.
