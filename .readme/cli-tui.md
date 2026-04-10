# CLI Modes: TUI and Log Streaming

RustyCAN ships with two headless CLI modes that require no GUI window.
Both modes use the same JSON configuration file as the `--config` flag
in GUI mode — no extra configuration is needed.

---

## `--tui` — Full-screen Terminal UI

```sh
rustycan --tui --config myconfig.json
```

Opens a full-screen terminal interface that replaces the GUI while
keeping all monitoring and control features accessible from the keyboard.
Useful on headless servers, remote SSH sessions, or when you prefer the
terminal.

### Panels

The TUI is divided into vertical sections (top → bottom):

| Panel       | Content                                                               |
|-------------|-----------------------------------------------------------------------|
| **NMT**     | One row per node: node-ID, EDS label, NMT state, heartbeat period    |
| **PDO**     | Live PDO signal values grouped by node and COB-ID                    |
| **SDO**     | Ring-buffer of the 50 most recent SDO transactions                   |
| **Log**     | Scrollable plain-text event log (toggle with `L`)                    |
| **Stats**   | FPS, bus load %, total frame count, and active log file path         |
| **Cmd bar** | Key-binding hints or active command input line                       |

### Key bindings

| Key       | Mode   | Action                                                  |
|-----------|--------|---------------------------------------------------------|
| `n`       | Normal | Enter **NMT** command input                             |
| `s`       | Normal | Enter **SDO read** command input                        |
| `w`       | Normal | Enter **SDO write** command input                       |
| `L`       | Normal | Toggle the event-log panel on / off                     |
| `q` / `Q` | Normal | Quit the TUI and restore the terminal                   |
| Ctrl-C    | Normal | Quit the TUI and restore the terminal                   |
| Esc       | Input  | Cancel the current command input and return to Normal   |
| Enter     | Input  | Submit the typed command and return to Normal           |
| Backspace | Input  | Delete the last character of the command buffer         |

### Interactive commands

#### NMT command — key `n`

Prompt: `NMT  <node> <command>`

| Example          | Effect                                         |
|------------------|------------------------------------------------|
| `1 start`        | Send Start Remote Node to node 1               |
| `2 stop`         | Send Stop Remote Node to node 2                |
| `0 reset_node`   | Broadcast Reset Node to all nodes              |
| `1 pre_op`       | Send Enter Pre-Operational to node 1           |
| `3 reset_comm`   | Send Reset Communication to node 3             |

Accepted command spellings (case-insensitive):
- **Start:** `start`
- **Stop:** `stop`
- **Pre-Operational:** `pre_op`, `preop`, `pre_operational`
- **Reset Node:** `reset_node`, `reset`
- **Reset Communication:** `reset_comm`, `reset_communication`

Node ID `0` broadcasts to all nodes.

#### SDO read — key `s`

Prompt: `SDO read  <node> <index_hex> <sub>`

| Example    | Effect                                                |
|------------|-------------------------------------------------------|
| `1 1000 0` | Upload Object 0x1000 sub 0 from node 1 (device type) |
| `2 6041 0` | Upload status word from node 2                        |

`index_hex` can be written with or without a `0x` prefix (e.g. `1000` or `0x1000`).

#### SDO write — key `w`

Prompt: `SDO write  <node> <index_hex> <sub> <hex_value>`

| Example         | Effect                                              |
|-----------------|-----------------------------------------------------|
| `1 6040 0 0006` | Download control word 0x0006 to node 1              |
| `2 6042 0 01f4` | Download target velocity 500 rpm to node 2          |

`hex_value` is written in hexadecimal (with or without `0x`).  The byte
length is inferred from the number of hex digits, rounded up to the nearest
byte, up to 8 bytes.

---

## `--log-to-stdout` — Stdout Event Streaming

```sh
rustycan --log-to-stdout --config myconfig.json
rustycan --log-to-stdout --config myconfig.json | tee capture.log
rustycan --log-to-stdout --config myconfig.json | grep NMT
```

Decodes incoming CAN events and prints each one as a single timestamped
plain-text line to standard output.  No window or TUI is opened, making
it suitable for pipelines, log archiving, and scripting.

### Output format

```
[HH:MM:SS.mmm] <TYPE>  <details>
```

#### Examples

```
[14:03:22.841] NMT    node   1  state OPERATIONAL
[14:03:22.843] PDO    node   1  cob 0x181  velocity = 1234  torque = 0.50
[14:03:22.851] SDO    node   1  READ   1000:00  device_type = 0x00020192
[14:03:22.900] SDO    node   2  WRITE  6040:00  control_word = pending
[14:03:23.001] DBC    id 0x181  MotorControl  setpoint=1500.0000  mode=3.0000
[14:03:23.102] ERROR  Failed to open adapter: device not found
# adapter disconnected
```

The stream exits when:
- The CAN adapter thread disconnects (cable pulled / adapter error), or
- The user sends Ctrl-C (SIGINT).

A `# adapter disconnected` or `# interrupted` line is printed on exit.

---

## Combining with `--http-port`

Both CLI modes are fully compatible with the `--http-port` flag.  The live
browser dashboard remains available at `http://127.0.0.1:<port>/` even in
TUI or log-streaming mode.

```sh
rustycan --tui --config myconfig.json --http-port 9090
```

---

## Notes

- Both `--tui` and `--log-to-stdout` require `--config`.  Launching without
  it will print an error and exit with a non-zero code.
- The TUI does **not** open an HTTP/SSE server by default (the port arg is
  accepted but the server is not yet started in headless modes).  Use the
  GUI (`rustycan --config myconfig.json`) if you need both the TUI and the
  live dashboard simultaneously.
- JSONL event logging (to the file specified in the config) continues
  normally in all modes.
