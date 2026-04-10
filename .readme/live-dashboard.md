# Live HTTP Dashboard

RustyCAN embeds a lightweight HTTP server that streams every decoded CAN event
to a browser in real time.  The dashboard is available at
**`http://localhost:7878/`** from the moment the app launches — no
configuration required.

## Accessing the Dashboard

1. Launch RustyCAN (the HTTP server starts automatically).
2. Open any browser on the same machine:
   ```
   http://localhost:7878/
   ```
3. Click **Connect** in the RustyCAN GUI to start a CAN session.
4. Events appear in the browser immediately.

> **Note:** The server binds exclusively to `127.0.0.1` (loopback) and is
> never reachable from other machines on your network.

## Dashboard Layout

The page adapts to macOS light and dark system appearance automatically via
`prefers-color-scheme`.

### NMT Node Grid

A card per configured node showing:

| Field | Description |
|---|---|
| Node ID | CANopen node identifier |
| EDS label | Filename of the loaded EDS (or "(no EDS)") |
| State badge | Colour-coded NMT state |
| Age | Seconds since last heartbeat |

State badge colours:

| State | Colour |
|---|---|
| Operational | Green |
| Pre-Operational | Amber |
| Stopped | Red |
| Bootup | Purple |
| Unknown | Grey |

### Event Log

Newest events appear at the top; the log keeps the last 200 entries.

Columns: **Time** · **Type** · **Node** · **Detail**

Row colours match event types:

| Type | Colour |
|---|---|
| `NMT_STATE` | Green tint |
| `NMT_COMMAND` / `NMT_COMMAND_SENT` | Amber tint |
| `SDO_READ` | Blue tint |
| `SDO_WRITE` | Purple tint |
| `PDO` | Orange tint |
| `session_start` | Neutral grey |

#### Filter buttons

Click **NMT**, **SDO**, or **PDO** to show only that category.
Click **All** to restore the full stream.

#### Pause / Resume

Click **⏸ Pause** to freeze the log display without disconnecting from the
stream.  Events that arrive while paused are discarded (the file log is
unaffected).  Click **▶ Resume** to continue.

## Raw SSE Stream

Every event is also available as a raw
[Server-Sent Events](https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events)
stream at `/events`.  Each `data:` message is a single JSONL line identical
to what is written to the log file — see [JSONL Log Format](jsonl-format.md)
for the full schema.

```bash
# Terminal: tail the live stream
curl http://localhost:7878/events

# Python example
import json, sseclient, requests
for event in sseclient.SSEClient(requests.get("http://localhost:7878/events", stream=True)):
    print(json.loads(event.data))
```

```js
// Browser / Node.js
const es = new EventSource("http://localhost:7878/events");
es.onmessage = e => console.log(JSON.parse(e.data));
```

### Auto-reconnect

The dashboard reconnects automatically if the stream is interrupted (e.g.,
RustyCAN is restarted).  The connection status dot in the header bar reflects
the current state:

| Dot | Meaning |
|---|---|
| Green | Connected — events are streaming live |
| Amber (pulsing) | Reconnecting — retrying with exponential back-off (1 s → 16 s) |
| Red | Disconnected — RustyCAN is not running |

## Multiple Clients

Any number of browser tabs or `curl` sessions can connect simultaneously.
Each subscriber receives an independent copy of the stream; a slow client
cannot delay or block other clients.  Clients that fall more than 128 events
behind will have the oldest buffered events dropped silently — the stream
continues without disconnection.

## Technical Details

| Property | Value |
|---|---|
| Port | 7878 |
| Bind address | `127.0.0.1` (loopback only) |
| Protocol | HTTP/1.1 with `Transfer-Encoding: chunked` |
| Content type (`/`) | `text/html; charset=utf-8` |
| Content type (`/events`) | `text/event-stream` |
| SSE keep-alive | 15 s (prevents proxy timeouts) |
| Broadcast buffer | 128 events per subscriber |
| HTML delivery | Embedded in binary via `include_str!` |
| Runtime | Dedicated `std::thread` + single-threaded tokio runtime |

The HTTP server shares no threads with the eframe render loop or the CAN
recv thread — it cannot stall the GUI or miss frames.
