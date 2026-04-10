# RustyCAN

[![Release](https://github.com/kodezine/RustyCAN/actions/workflows/release.yml/badge.svg)](https://github.com/kodezine/RustyCAN/actions/workflows/release.yml)
[![Latest release](https://img.shields.io/github/v/release/kodezine/RustyCAN)](https://github.com/kodezine/RustyCAN/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey)](#installation)

A native cross-platform GUI for monitoring, decoding, and controlling CANopen networks.

Connect a **PEAK PCAN-USB** adapter or a **KCAN Dongle** (STM32H753ZI-based, built with Embassy), optionally provide EDS device-description files, and get live NMT state, PDO signal values, and SDO transactions — all stored to a newline-delimited JSON log.

The **KCAN Dongle** is the project's own first-class hardware target: a custom USB CAN adapter with hardware timestamps, a fully documented binary protocol, and a path to Phase 3 hardware-level encryption (STM32H563, TrustZone).

## 🚀 Quick Start

### Installation

| Platform | Artifact | Guide |
|---|---|---|
| 🍎 **macOS** (Apple Silicon & Intel) | `.dmg` · Homebrew tap | [install-macos.md](.readme/install-macos.md) |
| 🪟 **Windows** (x86-64) | NSIS installer `.exe` | [install-windows.md](.readme/install-windows.md) |
| 🐧 **Linux** (x86-64) | AppImage · `.deb` | [install-linux.md](.readme/install-linux.md) |

> **Quick install on macOS:**
> ```sh
> brew tap kodezine/rustycan && brew install --cask rustycan
> ```

[📦 All release artifacts →](https://github.com/kodezine/RustyCAN/releases/latest)

### Build from Source

```sh
git clone https://github.com/kodezine/RustyCAN
cd RustyCAN
cargo run --release -p rustycan
```

The GUI window opens immediately. See [Building from Source](.readme/building.md) for development details.

## ✨ Key Features

- 🖥️ **Native GUI** — egui/eframe window, no terminal required
- 🔌 **Dual adapter support** — PEAK PCAN-USB or KCAN Dongle (STM32H753ZI)
- ⏱️ **Hardware timestamps** — µs-precision timestamps from FDCAN TIM2 (KCAN only)
- 💓 **NMT monitoring & control** — Live node states with broadcast/per-node commands
- 📊 **PDO & SDO decoding** — Live signal values with EDS support (optional)
- 🚗 **DBC signal decoding** — Full DBC support alongside CANopen
- 📝 **JSONL logging** — Every frame logged with structured JSON
- 🌐 **Live browser dashboard** — `http://localhost:7878/` streams events via SSE; NMT node grid + colour-coded event log; works in any browser

[→ View complete feature list](.readme/features.md)

## 📚 Documentation

Comprehensive documentation is organized in the [`.readme/`](.readme/) directory:

### Getting Started
- [Prerequisites](.readme/prerequisites.md) — Hardware adapters & Rust toolchain
- [Building from Source](.readme/building.md) — Build & development workflow
- [GUI Guide](.readme/gui-guide.md) — Connect & Monitor screens walkthrough

### Reference
- [CLI Configuration File](.readme/cli-config.md) — `--config` flag, JSON schema, auto-connect
- [JSONL Log Format](.readme/jsonl-format.md) — Complete logging specification
- [Live HTTP Dashboard](.readme/live-dashboard.md) — Browser-based live event viewer
- [DBC Signal Decoding](.readme/dbc-signal-decoding.md) — DBC file support details
- [Project Structure](.readme/project-structure.md) — Codebase organization
- [Features](.readme/features.md) — Complete feature list & status

### Advanced Topics
- [Multi-Node Stability](.readme/multi-node-stability.md) — Using 7-10+ nodes, error recovery
- [Logging Performance](.readme/logging-performance.md) — High-traffic optimization
- [Testing](.readme/testing.md) — Running tests & verification

## 💡 30-Second Demo

1. Launch RustyCAN
2. Select your adapter (PEAK PCAN-USB or KCAN Dongle)
3. Optionally add CANopen nodes with EDS files
4. Click **Connect**
5. Watch live NMT states, PDO signals, and SDO transactions
6. All frames logged to timestamped JSONL file

See the [GUI Guide](.readme/gui-guide.md) for detailed usage instructions.

## 🤝 Contributing

Pull requests welcome! For major changes, please open an issue first to discuss what you'd like to change.

## 📄 License

Licensed under either of [Apache-2.0](LICENSE) or MIT at your option.
