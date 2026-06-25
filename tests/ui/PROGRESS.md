# RustyCAN UI Testing Progress

Cross-session, cross-system progress tracker.
Update checkboxes and push to share status with other sessions/systems.

**GitHub Issue:** https://github.com/kodezine/RustyCAN/issues/77

**Phases:**
- Phase 0 — Infrastructure ✅ (this file + stubs + Playwright scaffold)
- Phase 1 — egui automated tests (widget snapshots + logic) ✅ macOS ✅ Fedora ✅ Windows
- Phase 2 — TUI automated tests (ratatui TestBackend) ✅ macOS ✅ Fedora ✅ Windows
- Phase 3 — Web automated tests (Playwright + mock SSE) ✅ macOS ✅ Fedora ✅ Windows
- Phase 4 — CI integration (`.github/workflows/ci.yml`) ✅
- Phase 5 — Manual test sessions per OS (screenshots/terminal/browser)

**Test systems:**
| Label | System | Access |
|-------|--------|--------|
| mac | macOS (this machine) | native |
| win | Windows 11 (Parallels on second Mac) | native VM |
| fed | Fedora latest | SSH; TUI direct, Web via port-forward |

---

## Phase 0 — Infrastructure

| Item | Status |
|------|--------|
| `egui_kittest` + `insta` added to `host/Cargo.toml` dev-deps | ✅ |
| `host/tests/ui_tui.rs` stub created | ✅ |
| `host/tests/ui_egui.rs` stub created | ✅ |
| `tests/ui/playwright/` scaffold created | ✅ |
| `tests/ui/PROGRESS.md` created (this file) | ✅ |
| GitHub Issue created and linked above | ✅ |
| `cargo check -p rustycan` passes | ✅ |
| Fedora display situation confirmed (`$DISPLAY` / Xvfb check) | ✅ Xvfb + scrot installed, smoke-test passed |

---

## Phase 1 — egui Automated Tests

> **Commands:**
> ```sh
> # Run all Phase 1 tests:
> cargo test -p rustycan --lib            # inline gui::tests (logic + snapshots)
> cargo test -p rustycan --test ui_egui   # integration-level API tests
>
> # Generate/update baseline snapshots on a new OS:
> UPDATE_SNAPSHOTS=1 cargo test -p rustycan --lib snapshot
> ```

### Pure logic (no display required)

| Test | mac | win | fed |
|------|-----|-----|-----|
| `bus_load_zones` at 0 % | ✅ | ✅ | ✅ |
| `bus_load_zones` at 30 % | ✅ | ✅ | ✅ |
| `bus_load_zones` at 70 % | ✅ | ✅ | ✅ |
| `bus_load_zones` at 100 % | ✅ | ✅ | ✅ |
| `bus_load_zones` total always 20 | ✅ | ✅ | ✅ |
| Duplicate node IDs detected | ✅ | ✅ | ✅ |
| Unique node IDs pass | ✅ | ✅ | ✅ |
| Node-ID parse: decimal | ✅ | ✅ | ✅ |
| Node-ID parse: `0x` hex | ✅ | ✅ | ✅ |
| Node-ID parse: `H`-suffix hex | ✅ | ✅ | ✅ |
| Node-ID parse: invalid input | ✅ | ✅ | ✅ |
| `AppState` initialises clean | ✅ | ✅ | ✅ |
| `AppState.init_nodes` populates map | ✅ | ✅ | ✅ |
| `AppState.init_nodes` is idempotent | ✅ | ✅ | ✅ |

### Widget snapshots (`egui_kittest::Harness` — wgpu offscreen renderer)

> **Windows note:** Parallels ARM requires `$env:WGPU_BACKEND="gl"` before running snapshot tests (DX12 crashes in the VM; OpenGL/ANGLE works fine).

| Test | mac | win | fed |
|------|-----|-----|-----|
| `bus_load_bar` at 0 % | ✅ | ✅ | ✅ |
| `bus_load_bar` at 55 % | ✅ | ✅ | ✅ |
| `bus_load_bar` at 85 % | ✅ | ✅ | ✅ |
| Connect screen — no dongle | Phase 1b | Phase 1b | Phase 1b |
| Connect screen — dongle detected | Phase 1b | Phase 1b | Phase 1b |
| Monitor NMT panel — 3 nodes | Phase 1b | Phase 1b | Phase 1b |

---

## Phase 2 — TUI Automated Tests

> **Commands:**
> ```sh
> # Run inline tests (parser + widget render):
> cargo test -p rustycan --lib tui
>
> # Run integration test (ring-buffer):
> cargo test -p rustycan --test ui_tui
>
> # Fedora headless:
> LIBGL_ALWAYS_SOFTWARE=1 xvfb-run -s "-screen 0 1920x1080x24" cargo test -p rustycan --lib tui
>
> # Windows Parallels ARM:
> $env:WGPU_BACKEND="gl"; cargo test -p rustycan --lib tui
> ```
>
> **Note:** widget + parser tests live inline in `tui/widgets.rs` / `tui/mod.rs` (pub(crate) API).
> `ui_tui.rs` only tests the public `AppState` ring-buffer API.

| Test | mac | win | fed |
|------|-----|-----|-----|
| NMT panel — node rows render | ✅ | ✅ | ✅ |
| NMT panel — Operational is green | ✅ | ✅ | ✅ |
| PDO panel — signal names + values | ✅ | ✅ | ✅ |
| SDO log — ring-buffer caps at 50 | ✅ | ✅ | ✅ |
| Stats bar — FPS / load / frames | ✅ | ✅ | ✅ |
| NMT cmd parse: `"1 start"` | ✅ | ✅ | ✅ |
| NMT cmd parse: `"0 pre_op"` broadcast | ✅ | ✅ | ✅ |
| SDO read parse: `"1 1000 0"` | ✅ | ✅ | ✅ |
| SDO write parse: `"1 6040 0 0006"` | ✅ | ✅ | ✅ |
| Invalid input — no panic | ✅ | ✅ | ✅ |

---

## Phase 3 — Web Automated Tests

> **Commands:**
> ```sh
> cd tests/ui/playwright
> npm install && npx playwright install chromium   # first time only
> npx playwright test
> ```
>
> **Architecture:** `mock-server.ts` starts a per-test Node.js HTTP server that
> serves the real `host/assets/index.html` and exposes an SSE `/events`
> endpoint. Tests call `mock.inject(event)` to push synthetic events without
> any live CAN hardware or Rust binary.

| Test | mac | win | fed |
|------|-----|-----|-----|
| Badge "Live" (green) when SSE stream is open | ✅ | ✅ | ✅ |
| Badge "Reconnecting…" on SSE drop | ✅ | ✅ | ✅ |
| Badge recovers to "Live" after SSE reconnect | ✅ | ✅ | ✅ |
| NMT grid — Operational node card | ✅ | ✅ | ✅ |
| NMT grid — Pre-Op / Stopped / Bootup colours | ✅ | ✅ | ✅ |
| NMT grid — label + age string | ✅ | ✅ | ✅ |
| NMT grid — sorted by node ID | ✅ | ✅ | ✅ |
| Event log — SDO_READ row columns | ✅ | ✅ | ✅ |
| Event log — NMT_STATE type badge | ✅ | ✅ | ✅ |
| Event log — capped at 200 rows | ✅ | ✅ | ✅ |
| Filter — SDO hides NMT rows | ✅ | ✅ | ✅ |
| Filter — All restores hidden rows | ✅ | ✅ | ✅ |
| Pause — stops log updates | ✅ | ✅ | ✅ |
| Resume — flushes buffered events | ✅ | ✅ | ✅ |
| Dark mode — CSS variables switch | ✅ | ✅ | ✅ |

---

## Phase 4 — CI Integration

| Item | Status |
|------|--------|
| `ci.yml` runs `ui_tui` test on all 3 OS | ✅ |
| `ci.yml` runs `ui_egui` test on all 3 OS | ✅ |
| `ci.yml` has `web-tests` job (Playwright, ubuntu-latest) | ✅ |
| Snapshots committed for mac | ✅ |
| Snapshots committed for win | ✅ |
| Snapshots committed for fed | ✅ |

---

## Phase 5 — Manual Test Sessions

### egui GUI Screenshots
> Naming: `egui_{screen}_{os}_{YYYY-MM-DD}.png` — upload to GitHub Issue as comment, tick box here.

| Screen | mac | win | fed |
|--------|-----|-----|-----|
| Connect screen — no dongle | ✅ | [ ] | [ ] |
| Connect screen — dongle detected | ✅ | [ ] | [ ] |
| Connect screen — validation error | [ ] | [ ] | [ ] |
| Monitor — NMT panel | ✅ | [ ] | [ ] |
| Monitor — PDO panel | ✅ | [ ] | [ ] |
| Monitor — SDO log | [ ] | [ ] | [ ] |
| Monitor — DBC signals panel | [ ] | [ ] | [ ] |
| Monitor — full status bar | ✅ | [ ] | [ ] |
| Plot view window | [ ] | [ ] | [ ] |
| PEAK adapter shown (mac/win only) | ✅ | [ ] | N/A |
| PEAK UI hidden on Linux | N/A | N/A | [ ] |

### TUI — Terminal Session
> Run: `cargo run -p rustycan -- --tui --config host/config.example.json`
> On Fedora: run in SSH session and share terminal with Copilot.

| Panel / behaviour | mac | win | fed |
|-------------------|-----|-----|-----|
| NMT panel renders with nodes | [ ] | [ ] | [ ] |
| PDO panel renders with signals | [ ] | [ ] | [ ] |
| SDO log panel renders | [ ] | [ ] | [ ] |
| Event log toggles with `L` | [ ] | [ ] | [ ] |
| Stats bar shows FPS + load | [ ] | [ ] | [ ] |
| NMT command input (`n` key) | [ ] | [ ] | [ ] |
| SDO read input (`s` key) | [ ] | [ ] | [ ] |
| SDO write input (`w` key) | [ ] | [ ] | [ ] |
| Quit with `q` | [ ] | [ ] | [ ] |

### Web Dashboard — Browser
> Run app, open http://localhost:7878 (Fedora: `ssh -L 7878:localhost:7878 <fedora-host>` then open locally)

| Section | mac | win | fed |
|---------|-----|-----|-----|
| Connection badge — Stream: Live/Offline | ✅ | [ ] | [ ] |
| Adapter badge — Disconnected/Ready/Capturing | ✅ | [ ] | [ ] |
| NMT node grid renders | ✅ | [ ] | [ ] |
| Event log scrolls | ✅ | [ ] | [ ] |
| Filter buttons work | [ ] | [ ] | [ ] |
| Pause / resume works | [ ] | [ ] | [ ] |
| Dark mode appearance | ✅ | [ ] | [ ] |
| Mobile-width layout (devtools) | [ ] | [ ] | [ ] |

---

## Completion Gate

All boxes above ticked → close GitHub Issue → remove `tests/ui/` directory.
