# bbd — BinaryBlockDownload Firmware Update Tool

`bbd` is a command-line CANopen firmware update tool built into the RustyCAN workspace. It downloads binary block files to CANopen bootloader nodes via SDO transfers, using either a **PEAK PCAN-USB** adapter or a **KCAN Dongle**.

It is a faithful Rust port of the C `BinaryBlockDownload` tool from the CANopen bootloader toolchain.

> **Packaging note**: `bbd` is built alongside `rustycan` but is not included in the packaged release artifacts (DMG / NSIS / AppImage). It is currently a developer/integrator tool only.

---

## Prerequisites

- One of:
  - **PEAK PCAN-USB** adapter with the [PEAK driver library](https://mac-can.com) installed (`libPCBUSB.dylib` on macOS, `PCANBasic.dll` on Windows).
  - **KCAN Dongle** (STM32H753ZI) connected via USB — no driver installation required.
- A CANopen device running a compatible CANopen bootloader.
- A binary block file (`.bin`) produced by the bootloader toolchain.

---

## Building

`bbd` is part of the `rustycan` package and builds automatically with:

```sh
cargo build --bin bbd
# or release build:
cargo build --release --bin bbd
```

The binary is placed in `target/debug/bbd` or `target/release/bbd`.

---

## Usage

```
bbd [OPTIONS] <FILE>
```

### Minimal Example — update firmware on node 5

```sh
bbd -N 5 firmware.bin
```

### KCAN Dongle

```sh
bbd --adapter kcan --node-id 5 firmware.bin
```

### Block-mode SDO transfer (faster for large files)

```sh
bbd --node-id 5 --sdoc-type 2 firmware.bin
```

### Load bootloader-update application

```sh
bbd --node-id 5 --blupdate-app bootloader_update_app.bin
```

### Update the bootloader (requires blupdate-app already running)

```sh
bbd --node-id 5 --blupdate new_bootloader.bin
```

### Custom CAN ID bases and timeout

```sh
bbd --node-id 5 --tx-baseid 0x600 --rx-baseid 0x580 --timeout 1000 firmware.bin
```

---

## CLI Reference

| Flag | Default | Description |
|---|---|---|
| `<FILE>` | *(required)* | Binary block file to download |
| `-n / -N, --node-id <NUM>` | `1` | Target CANopen node ID (1–127) |
| `-p / -P, --program-number <NUM>` | `1` | Program slot number (subindex of 0x1F50/0x1F51/0x1F57) |
| `--timeout <MS>` | `500` | SDO response timeout in milliseconds |
| `--delay <100US>` | `500` | Delay between flash-status polls in 100 µs units (500 = 50 ms) |
| `--delay-check-app <100US>` | `20000` | Delay before checking if the application started (20000 = 2 s) |
| `--delay-check-bl <100US>` | `20000` | Delay before checking if the bootloader re-entered (20000 = 2 s) |
| `--sdoc-type <TYPE>` | `0` | SDO transfer mode: `0` = segmented, `2` = block |
| `--repeats <NUM>` | `200000` | Maximum retry count for flash-BUSY polling |
| `--repeats-crc <NUM>` | `10000` | Maximum retry count for flash-CRC-BUSY polling |
| `--tx-baseid <HEX>` | `0x600` | SDO request COB-ID base (master → node) |
| `--rx-baseid <HEX>` | `0x580` | SDO response COB-ID base (node → master) |
| `--vendor-id <HEX>` | `0x0` | Vendor ID to validate (0 = skip check) |
| `--product-code <HEX>` | `0x0` | Product code to validate (0 = skip check) |
| `--total-steps <NUM>` | `0` | Total steps in a multi-step sequence (progress display) |
| `--current-step <NUM>` | `0` | Current step in a multi-step sequence (progress display) |
| `--blupdate-app` | *(off)* | Load the bootloader-update application |
| `--blupdate` | *(off)* | Update the bootloader (requires blupdate-app already running) |
| `--adapter <ADAPTER>` | `peak` | Adapter backend: `peak` or `kcan` |
| `--port <PORT>` | `1` | Adapter port / channel (PEAK: channel number; ignored for KCAN when `--kcan-serial` is set) |
| `--baud <BPS>` | `500000` | CAN bus baud rate in bits per second |
| `--kcan-serial <SERIAL>` | *(none)* | KCAN dongle USB serial (optional; first found if omitted) |

---

## SDO Transfer Modes

| `--sdoc-type` | Mode | Description |
|---|---|---|
| `0` | Segmented | Standard CiA 301 segmented download. Compatible with all CANopen bootloaders. Each 7-byte CAN frame is individually acknowledged. Suitable for any payload size. |
| `2` | Block | CiA 301 block download. Multiple segments per block, acknowledged as a group with CRC. Higher throughput for large firmware files. Requires bootloader support. |

Use `--sdoc-type 2` for faster downloads with large files if the target bootloader supports block transfers.

---

## Binary Block File Format

The tool reads the binary block format used by the CANopen bootloader toolchain. The file is a sequence of blocks with no file header:

```
┌─────────────────┐
│ block_num  u32  │  4 bytes, little-endian
├─────────────────┤
│ flash_addr u32  │  4 bytes, little-endian  (target flash address)
├─────────────────┤
│ data_size  u32  │  4 bytes, little-endian  (payload byte count)
├─────────────────┤
│ data      [u8]  │  data_size bytes         (firmware payload)
├─────────────────┤
│ crc32      u32  │  4 bytes, little-endian  (CRC over entire block)
└─────────────────┘
```

The **entire block** (header + payload + CRC) is sent verbatim as the SDO data payload to object `0x1F50 / program_number`. The bootloader validates the embedded CRC and flash address internally.

---

## Download Sequence

`bbd` implements the full CANopen bootloader flash programming protocol:

| Step | State | CANopen SDO Operation |
|---|---|---|
| 1 | CheckBootloader | Read `0x1000/0` — compare device type to `0x10000000` |
| 2 | StartBootloader | Write `0x80` to `0x1F51/prog_no` (if bootloader not yet active) |
| 3 | CheckVendorId | Read `0x1018/1` — compare to `--vendor-id` (if non-zero) |
| 4 | CheckProductCode | Read `0x1018/2` — compare to `--product-code` (if non-zero) |
| 5 | ClearFlash | Write `0x03` to `0x1F51/prog_no` |
| 6 | WaitClear | Poll `0x1F57/prog_no` until `0x00` (OK) |
| 7 | Download (loop) | Write each block to `0x1F50/prog_no`, then poll `0x1F57` |
| 8 | FirstStartApp | Write `0x01` to `0x1F51/prog_no` |
| 9 | CheckAppWorks | Read `0x1000/0` — if not bootloader device type → **success** |
| 10* | SetSignature | Write `0x83` to `0x1F51/prog_no` (if app did not start) |
| 11* | WaitSetSignature | Poll `0x1F57/prog_no` until `0x00` |
| 12* | FinalStartApp | Write `0x01` to `0x1F51/prog_no` again |
| 13* | FinalCheckApp | Read `0x1000/0` — verify application running |

*Steps 10–13 are the "set signature" fallback path taken when the application does not start on the first attempt.

---

## Flash Status Codes (object 0x1F57)

| Value | Meaning |
|---|---|
| `0x00000000` | OK — operation complete |
| `0x00000001` | BUSY — programming in progress |
| `0x00000006` | CRC-BUSY — CRC verification in progress |
| Any other | Error — download aborted with the raw status code |

---

## Error Codes

| Error | Description |
|---|---|
| SDO timeout | No response from the target node within `--timeout` ms |
| SDO abort | Target returned an SDO abort frame (abort code printed in hex) |
| Vendor ID mismatch | Device vendor ID does not match `--vendor-id` |
| Product code mismatch | Device product code does not match `--product-code` |
| Flash BUSY timeout | Flash status remained BUSY beyond `--repeats` retries |
| Flash CRC-BUSY timeout | Flash CRC check exceeded `--repeats-crc` retries |
| App start failed | Application did not start after the full signature/retry sequence |
| Invalid format | The binary block file is truncated or malformed |

---

## Exit Codes

| Code | Meaning |
|---|---|
| `0` | Firmware download successful |
| `1` | Download failed (error printed to stderr) |

---

## Architecture

```
host/src/bin/bbd/
├── main.rs           — CLI parsing (clap), adapter open, progress output
├── sdo_client.rs     — Synchronous SDO master: read_u32, write_u32, download
├── state_machine.rs  — Firmware download FSM (port of FtCop.c state machine)
└── file.rs           — Binary block file iterator
```

All adapter access goes through the same [`CanAdapter`](../host/src/adapters/mod.rs) trait used by the main `rustycan` application. SDO encode/decode functions are reused from [`canopen/sdo.rs`](../host/src/canopen/sdo.rs).
