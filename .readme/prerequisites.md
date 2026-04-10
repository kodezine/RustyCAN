# Prerequisites

## Hardware

RustyCAN supports two adapters; at least one is required.

### Option A — PEAK PCAN-USB

A **PEAK PCAN-USB** adapter with the macOS PCANBasic library:

1. Download the latest *PCUSB* `.pkg` from **<https://mac-can.com>**.
2. Run the installer — it places `libPCBUSB.dylib` in `/usr/local/lib/`.
3. Connect your PCAN-USB adapter; it appears as channel `1` by default.

### Option B — KCAN Dongle (STM32H753ZI Nucleo)

The KCAN Dongle is the project's own hardware target. Flash the Embassy
firmware onto a **NUCLEO-H753ZI** board:

```sh
# Install the target and flashing tool
rustup target add thumbv7em-none-eabihf
cargo install probe-rs-tools

# Build and flash
cd firmware
cargo run --release -p dongle-h753
```

Connect the board's **CN5 Micro-B USB** port to the host; it enumerates as
VID `0x1209` / PID `0xBEEF` ("KCAN Dongle v1"). Wiring for CAN:

| Nucleo pin | Signal | TJA1051T |
|------------|--------|----------|
| PD0 | FDCAN1 RX | RXD |
| PD1 | FDCAN1 TX | TXD |
| 3V3 | VCC | VCC |
| GND | GND | GND |

## 🦀 Rust toolchain

```sh
rustup update stable  # MSRV: 1.85+
```
