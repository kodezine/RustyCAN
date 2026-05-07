# KCAN Dongle Firmware Updates

RustyCAN can update KCAN Dongle firmware directly over USB — no ST-Link, no `dfu-util`, no driver
installation. Updates are applied using standard USB DFU Class 1.1 backed by `embassy-boot-stm32`
with A/B bank partitioning and Ed25519 image signing.

---

## User Experience

### GUI

When a connected dongle runs firmware older than the version available (bundled in the host release
or newer on GitHub), a non-blocking banner appears in the Monitor screen:

> **Dongle v1.0.0 → v1.2.0 available.** `[Update Now]` `[Later]`

Clicking **Update Now** shows a progress bar while the image transfers. On completion the device
reconnects automatically and resumes normal operation.

### TUI

A status line is shown after connection:

```
[FIRMWARE] v1.0.0 → v1.2.0 available (y/N)
```

### CLI (`--config` mode)

```
[FIRMWARE] Update available: v1.0.0 → v1.2.0. Pass --update-firmware to apply.
```

The `--update-firmware` flag runs the full DFU sequence non-interactively, printing each step, then
reconnects.

### Configuration

The default behaviour is `"notify"` — the user is always asked before anything is flashed.

```json
"firmware_updates": "notify"
```

| Value | Behaviour |
|---|---|
| `"notify"` | Show prompt; update only after user confirms *(default)* |
| `"auto"` | Update silently without prompting |
| `"disabled"` | Never check or update |

---

## Firmware Source

RustyCAN bundles firmware binaries matching the current host release (`host/assets/firmware/`). At
connect time, the host also checks the GitHub Releases API in a background thread (non-blocking) for
any newer release. If a newer signed binary is available it is downloaded on demand and preferred
over the bundled one. If the network is unreachable, the bundled binary is used silently.

---

## Security Model

### Two-Layer Ed25519 Signature Verification

All KCAN firmware images are signed with an Ed25519 private key held exclusively in the GitHub
Actions Secret `KCAN_SIGNING_KEY`. The corresponding 32-byte public key is committed to the
repository at `firmware/signing-pubkey.bin` and embedded at compile time into both the bootloader
(immutable without JTAG) and the host application.

**Layer 1 — Host verification (before initiating DFU):**
The host reads the `.bin.signed` file (raw binary + appended 64-byte Ed25519 signature), verifies
it against the embedded public key, and only proceeds if verification passes. An invalid or missing
signature results in a hard abort — DFU is never initiated and the user sees an error.

**Layer 2 — Bootloader verification (before accepting the image swap):**
After the DFU transfer completes, `embassy-boot-stm32`'s `verify` feature re-checks the Ed25519
signature of the received image against the public key compiled into the bootloader. If verification
fails the swap is aborted, the DFU partition is marked bad, and the device reboots on the existing
firmware untouched.

This means even if someone bypasses the host application entirely and uses raw `dfu-util` or custom
USB code, the bootloader rejects unsigned images. The device cannot be corrupted without possession
of the private key.

### Key Rotation

Rotating the signing key requires reflashing the bootloader via JTAG (ST-Link / probe-rs) with a
new embedded public key. This is intentional: the trust anchor cannot be changed remotely, which
prevents an attacker from rotating the key via a firmware update.

### Signed Image Format

```
┌───────────────────────────────────────┐
│  Raw app binary  (N bytes)            │
├───────────────────────────────────────┤
│  Ed25519 signature  (64 bytes)        │
└───────────────────────────────────────┘
```

The host verifies and strips the trailing 64 bytes before streaming the raw payload over DFU.

---

## Flash Layout

Both H743XI and H753ZI carry 2 MB of flash with the same bank topology.

```
0x08000000 ┌─────────────────────────────────────────────┐
           │  Bootloader  (128 KB — Bank1 sector 0)      │
           │  • embassy-boot-stm32, USB DFU class        │
           │  • Ed25519 public key compiled in           │
           │  • Reads RTC_BKP0R for DFU_REQUESTED magic  │
           │  • Verifies + swaps DFU→Active on update    │
           │  • Reverts swap if mark_booted() not called │
0x08020000 ├─────────────────────────────────────────────┤
           │  Active Application  (896 KB — sects 1–7)  │
           │  • KCAN firmware (Embassy, as today)        │
           │  • USB DFU Runtime interface descriptor     │
           │  • EP0 0x20 ENTER_DFU_MODE handler          │
           │  • Calls mark_booted() after USB enumerate  │
0x08100000 ├─────────────────────────────────────────────┤
           │  DFU Partition  (1 MB — Bank2)              │
           │  • Receives new image during DFU transfer   │
           │  • Copied to Active by bootloader after     │
           │    successful verify + manifest             │
0x08200000 └─────────────────────────────────────────────┘
```

> **Boundary note:** The 128 KB bootloader allocation is set by a `cargo size` measurement of the
> `embassy-boot-stm32` + `ed25519-dalek` + USB DFU stack in the `bootloader-size-probe` crate
> (Phase 1, Step 1). If the bootloader grows beyond this budget the `memory.x` linker scripts for
> both the bootloader and app crates must be updated together.

---

## DFU Update Flow

```
Host (RustyCAN)                App Firmware              Bootloader
      │                             │                         │
      │── GET_INFO ────────────────►│                         │
      │◄─ fw version 1.0.0 ────────│                         │
      │                             │                         │
      │  [user confirms update]     │                         │
      │                             │                         │
      │── EP0 0x20 ENTER_DFU_MODE ─►│                         │
      │                        write RTC_BKP0R               │
      │                        sys_reset() ────────────────► │
      │                                                       │
      │  [wait up to 15 s for DFU re-enumeration]            │
      │                                                       │
      │── DFU_GETSTATUS ─────────────────────────────────────►│
      │◄─ dfuIDLE ───────────────────────────────────────────│
      │                                                       │
      │  [loop: DFU_DNLOAD 64-byte chunks + GETSTATUS]       │
      │── DFU_DNLOAD (block 0…N) ────────────────────────────►│
      │── DFU_DNLOAD (wLength=0) ────────────────────────────►│
      │── DFU_GETSTATUS ─────────────────────────────────────►│
      │◄─ dfuMANIFEST ──────────────────────────────────────│
      │                                          verify Ed25519
      │                                          copy DFU→Active
      │                                          mark_updated()
      │                                          sys_reset()
      │                                                       │
      │  [re-enumerates as KCAN app]                          │
      │── GET_INFO ────────────────►│                         │
      │◄─ fw version 1.2.0 ────────│                         │
      │                        mark_booted()                  │
```

---

## Rollback Safety

`embassy-boot` tracks whether the newly swapped firmware has confirmed successful startup. If the
new firmware panics, hard-faults, or fails to initialise USB before calling `mark_booted()`, the
bootloader detects the unconfirmed state on the next reset and **automatically reverts** to the
previous working firmware. The user's device is never left in an unrecoverable state due to a bad
update.

`mark_booted()` is called from `kcan_io_task` immediately after the `USB_CONFIGURED` signal fires
and the bulk endpoints are ready. USB readiness is the functional bar: firmware that boots but
whose USB stack hangs is treated as a failed update.

---

## Building & Flashing (Developers)

### One-time setup — generate signing keypair

```sh
# Generates a new keypair; writes public key to firmware/signing-pubkey.bin
cargo run --bin sign-firmware -- --generate
# Copy the printed private key into GitHub Actions Secret: KCAN_SIGNING_KEY
# Then commit firmware/signing-pubkey.bin
```

### Build and sign manually

```sh
# 1. Build firmware app (from firmware/ workspace root)
cd firmware
cargo build --release -p dongle-h743

# 2. Strip to raw binary
arm-none-eabi-objcopy -O binary \
  target/thumbv7em-none-eabihf/release/dongle-h743 \
  dongle-h743-app.bin

# 3. Sign (from repo root)
cd ..
cargo run --bin sign-firmware -- --key <hex-private-key> firmware/dongle-h743-app.bin
# Produces: firmware/dongle-h743-app.bin.signed
```

### Flash bootloader + app via probe-rs (initial provisioning)

```sh
cd firmware

# Bootloader goes to Bank1 sector 0
probe-rs download --base-address 0x08000000 \
  target/thumbv7em-none-eabihf/release/bootloader-h743

# App goes to Bank1 sector 1 (0x08020000)
probe-rs download --base-address 0x08020000 \
  target/thumbv7em-none-eabihf/release/dongle-h743
```

Subsequent updates can be applied through RustyCAN over USB without a probe-rs connection.

### Verify a signed binary

```sh
cargo run --bin sign-firmware -- --verify firmware/dongle-h743-app.bin.signed
# OK — signature valid, payload 114688 bytes
```

---

## Release Pipeline

Signing and binary bundling happen automatically in `.github/workflows/release.yml`:

1. Firmware ELFs are built for `thumbv7em-none-eabihf`
2. `arm-none-eabi-objcopy` strips each to a raw `.bin`
3. `sign-firmware` appends the Ed25519 signature using `KCAN_SIGNING_KEY` from Secrets
4. Signed `.bin.signed` files are copied into `host/assets/firmware/` before host packaging —
   the host installer always bundles the firmware from the same release
5. Both `.elf` and `.bin.signed` are published as release artifacts

`KCAN_H743_FW_VERSION` and `KCAN_H753_FW_VERSION` environment variables are injected into the
host build so `build.rs` embeds the bundled firmware version as a compile-time constant.
