# RustyCAN — Documentation

[![Release](https://github.com/kodezine/RustyCAN/actions/workflows/release.yml/badge.svg)](https://github.com/kodezine/RustyCAN/actions/workflows/release.yml)
[![Latest release](https://img.shields.io/github/v/release/kodezine/RustyCAN)](https://github.com/kodezine/RustyCAN/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](../LICENSE)

## 📦 Installation

Choose your operating system:

| Platform | Artifact | Build | Guide |
|---|---|---|---|
| 🍎 **macOS** (Apple Silicon & Intel) | `.dmg` via Homebrew tap | [![macOS](https://github.com/kodezine/RustyCAN/actions/workflows/release.yml/badge.svg?event=push)](https://github.com/kodezine/RustyCAN/actions/workflows/release.yml) | [install-macos.md](install-macos.md) |
| 🪟 **Windows** (x86-64) | NSIS installer `.exe` | [![Windows](https://github.com/kodezine/RustyCAN/actions/workflows/release.yml/badge.svg?event=push)](https://github.com/kodezine/RustyCAN/actions/workflows/release.yml) | [install-windows.md](install-windows.md) |
| 🐧 **Linux** (x86-64) | AppImage · `.deb` | [![Linux](https://github.com/kodezine/RustyCAN/actions/workflows/release.yml/badge.svg?event=push)](https://github.com/kodezine/RustyCAN/actions/workflows/release.yml) | [install-linux.md](install-linux.md) |

## 🔗 Quick links

- [⬇️ Releases](https://github.com/kodezine/RustyCAN/releases) — download artifacts directly
- [📖 README](../README.md) — project overview, GUI walkthrough, JSONL log format
- [🔌 Firmware](../firmware/) — KCAN Dongle (STM32H753ZI) build & flash instructions

## ✅ Build artifacts per release

Each tagged release produces these artifacts from the
[release workflow](https://github.com/kodezine/RustyCAN/actions/workflows/release.yml):

| Artifact | Platform | Notes |
|---|---|---|
| `rustycan-<ver>-aarch64-apple-darwin.dmg` | 🍎 macOS arm64 | Drag-to-Applications DMG |
| `rustycan-<ver>-x86_64-pc-windows-msvc.exe` | 🪟 Windows x86-64 | NSIS one-click installer |
| `rustycan-<ver>-x86_64-linux.AppImage` | 🐧 Linux x86-64 | Portable, no install needed |
| `rustycan_<ver>_amd64.deb` | 🐧 Debian/Ubuntu | `dpkg -i` installer with `.desktop` entry |
| `dongle-h753-<ver>.bin` | 🔌 Firmware | KCAN Dongle flash image (STM32H753ZI) |
