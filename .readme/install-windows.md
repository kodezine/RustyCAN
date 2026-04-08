# 🪟 Installing RustyCAN on Windows

[![Release](https://github.com/kodezine/RustyCAN/actions/workflows/release.yml/badge.svg)](https://github.com/kodezine/RustyCAN/actions/workflows/release.yml)

> **Platform:** Windows 10 (1903+) or Windows 11 · x86-64

---

## Download & install

| Step | Action | Expected result |
|---|---|---|
| 1️⃣ | Open the [⬇️ Releases page](https://github.com/kodezine/RustyCAN/releases) | — |
| 2️⃣ | Download **`rustycan-<version>-x86_64-pc-windows-msvc.exe`** | NSIS installer saved locally |
| 3️⃣ | Run the installer | Wizard opens |
| 4️⃣ | Click through; installs to `%LOCALAPPDATA%\Programs\RustyCAN` (no admin needed) | Progress bar completes |
| 5️⃣ | ✅ Start Menu shortcut created | Visible in Start → RustyCAN |
| 6️⃣ | ✅ Launch **RustyCAN** | GUI window opens |

> ⚠️ **SmartScreen warning:** Windows may show _"Windows protected your PC"_
> because the binary is not yet code-signed. Click **More info → Run anyway**.

---

## 🔌 Prerequisites

### KCAN Dongle — ✅ no extra drivers needed

Windows 10/11 includes WinUSB support for USB bulk devices. The KCAN Dongle
(VID `0x1209` / PID `0xBEEF`) enumerates automatically — no additional driver
installation is required.

### PEAK PCAN-USB adapter — optional

If you want to use a PEAK PCAN-USB adapter:

| Step | Action |
|---|---|
| 1️⃣ | Download the Windows PEAK driver from <https://peak-system.com/downloads> |
| 2️⃣ | Run the installer — registers `PCANBasic.dll` in the system |
| 3️⃣ | Connect the PEAK adapter; Windows assigns it a PCAN channel |
| 4️⃣ | Launch RustyCAN → select **PEAK PCAN-USB** on the Connect screen |

> ℹ️ If `PCANBasic.dll` is not found, RustyCAN shows a friendly message with
> the download URL — the app still opens and the KCAN Dongle path is unaffected.

---

## 🗑️ Uninstall

**Settings → Apps → Installed apps → RustyCAN → Uninstall**

or run:

```
%LOCALAPPDATA%\Programs\RustyCAN\Uninstall RustyCAN.exe
```

---

## 🚀 First launch

1. Open **RustyCAN** from the Start Menu or desktop shortcut.
2. On the **Connect** screen:
   - Choose **KCAN Dongle** or **PEAK PCAN-USB**
   - Set baud rate (default `250000`)
   - Optionally browse to `.eds` files for your nodes
3. Click **Connect** — the button activates automatically when the adapter is detected.

See the [README](../README.md#using-the-gui) for a full GUI walkthrough.

---

## 🛠 Troubleshooting

| Symptom | Fix |
|---|---|
| _"Windows protected your PC"_ | Click **More info → Run anyway** |
| KCAN Dongle not detected | Open Device Manager — check for `Unknown device` under USB; reinstall WinUSB via Zadig if needed |
| PEAK adapter not found | Verify `PCANBasic.dll` is installed: `where PCANBasic.dll` in CMD |
| App fails to start | Check Windows Event Viewer → Application log for the crash entry |
