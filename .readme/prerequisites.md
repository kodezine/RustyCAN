# Prerequisites

## Hardware

RustyCAN supports two adapters; at least one is required.

### Option A — PEAK PCAN-USB

A **PEAK PCAN-USB** adapter with the macOS PCANBasic library:

1. Download the latest *PCUSB* `.pkg` from **<https://mac-can.com>**.
2. Run the installer — it places `libPCBUSB.dylib` in `/usr/local/lib/`.
3. Connect your PCAN-USB adapter; it appears as channel `1` by default.

### Option B — KCAN Dongle (NUCLEO-H753ZI / dongle-h753)

The KCAN Dongle is the project's own hardware target. Two board variants are
supported; both enumerate identically over USB.

> **Note:** The firmware workspace contains multiple packages with different
> STM32 chip features. Always build with `-p <package>` — building the whole
> workspace (`cargo build`) is not supported and will fail with a
> "Multiple stm32xx Cargo features enabled" error from stm32-metapac.

```sh
rustup target add thumbv7em-none-eabihf
cargo install probe-rs-tools

cd firmware
cargo run --release -p dongle-h753
```

Connect **CN5 Micro-B USB** to the host. Wiring for CAN (external TJA1051T
or equivalent transceiver required):

| Nucleo pin | Signal | Transceiver |
|------------|--------|-------------|
| PD0 | FDCAN1 RX | RXD |
| PD1 | FDCAN1 TX | TXD |
| 3V3 | VCC | VCC |
| GND | GND | GND |

### Option C — KCAN Dongle (STM32H743I-EVAL MB1246 Rev E / dongle-h743)

```sh
rustup target add thumbv7em-none-eabihf
cargo install probe-rs-tools

# Must cd into the package directory — the per-package .cargo/config.toml
# (STM32H743XIHx) only applies when CWD is within firmware/dongle-h743/
cd firmware/dongle-h743
cargo run --release
```

Connect **CN18 Micro-AB USB** to the host for CAN-over-USB (OTG HS via ULPI,
480 Mb/s). CAN is available on the **CN3 DB9** connector via the on-board
TJA1044 transceiver — no external wiring required.
Debug/flash uses **C23 Micro-USB** (ST-LINK V3E). See
[`.readme/BRING_UP_PLAN.md`](BRING_UP_PLAN.md) for the full bring-up checklist.

Both dongles enumerate as VID `0x1209` / PID `0xBEEF`.

### USB MPS (`usb-hs` Cargo feature)

The bulk endpoint max packet size (MPS) is controlled by the `usb-hs` Cargo
feature in each dongle crate:

| Crate | Default | MPS | When to change |
|-------|---------|-----|----------------|
| `dongle-h743` | `usb-hs` **on** | 512 bytes | Use `--no-default-features` only if connecting via a FS-only hub or host |
| `dongle-h753` | `usb-hs` **off** | 64 bytes | Add `--features usb-hs` only for testing; h753 hardware is FS-only |

The h743 connects via USB OTG HS (ULPI, 480 Mb/s) and **must** advertise
MPS=512 — Windows rejects High-Speed devices that declare a 64-byte (FS) MPS.
The h753 connects via USB OTG FS (12 Mb/s) and uses 64-byte MPS; at this size
an 80-byte KCAN frame splits across two USB packets (64 + 16 bytes), which the
host accumulation buffer handles transparently.

The host adapter (`kcan.rs`) requires no matching configuration — its
`BULK_IN_BUF=512` is a multiple of both 64 and 512 and handles both layouts
automatically.

## 🦀 Rust toolchain

```sh
rustup update stable  # MSRV: 1.85+
```
