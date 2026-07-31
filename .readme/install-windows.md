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

### Summit adapter — optional

If you want to use a Summit adapter:

| Step | Action |
|---|---|
| 1️⃣ | Download the Windows Summit driver from <https://peak-system.com/downloads> |
| 2️⃣ | Run the installer — registers `PCANBasic.dll` in the system |
| 3️⃣ | Connect the Summit adapter; Windows assigns it a PCAN channel |
| 4️⃣ | Launch RustyCAN → select **Summit** on the Connect screen |

> ℹ️ If `PCANBasic.dll` is not found, RustyCAN shows a friendly message with
> the download URL — the app still opens and the KCAN Dongle path is unaffected.

### Apex USB-CAN adapter — one-time WinUSB binding (Zadig)

RustyCAN talks to the Apex module directly over USB (no vendor DLL). Unlike the
KCAN Dongle, the Apex firmware does **not** advertise WinUSB automatically, so
Windows will not load a usable driver on its own. Bind it to Microsoft's in-box
`winusb.sys` once, using the free [Zadig](https://zadig.akeo.ie/) tool — no
paid driver signing is involved.

> **Important — bind *both* device IDs.** During connect, RustyCAN boots the
> module from its bootloader into the application, and it re-enumerates with a
> different USB product ID. You must bind WinUSB to **both** the application and
> the bootloader IDs:
>
> | Role | VID | PID |
> |---|---|---|
> | Apex (application) | `0x0878` | `0x1181` |
> | Apex (bootloader) | `0x0878` | `0x1101` **or** `0x1122` |
>
> The bootloader PID differs by hardware generation — bind whichever one your
> unit shows in Zadig (check the USB ID field). If you bind only one, the
> connect sequence stalls the first time the device switches modes.

| Step | Action | Expected result |
|---|---|---|
| 1️⃣ | Download and run **Zadig** (portable `.exe`, no install) | Zadig window opens |
| 2️⃣ | Menu **Options → List All Devices** | Hidden/driverless devices appear |
| 3️⃣ | Select **USB-CANmodul1** and note its USB ID (`0878 1181`, `0878 1101`, or `0878 1122`) | USB ID shown |
| 4️⃣ | Set the target driver to **WinUSB**, click **Replace Driver** | "Driver installed successfully" |
| 5️⃣ | Make the *other* ID appear — replug and/or launch RustyCAN once (it boots the loader into the app `0878 1181`) — then repeat 3–4 for it | Both IDs now on WinUSB |
| 6️⃣ | Launch RustyCAN → select **Apex** on the Connect screen → **Connect** | Frames start flowing |

> **This replaces the vendor driver** for the Apex module, so the vendor's own
> Windows software can't use it while WinUSB is bound. It's fully reversible:
> **Device Manager → the device → Uninstall device** (tick *delete driver*) and
> replug, or use Zadig to restore.

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
   - Choose **KCAN Dongle**, **Summit**, or **Apex** (Apex needs the one-time WinUSB binding above)
   - Set baud rate (default `250000`)
   - Optionally browse to `.eds` files for your nodes
3. Click **Connect** — the button activates automatically when the adapter is detected.

See the [GUI Guide](gui-guide.md) for a full GUI walkthrough.

---

## 🛠 Troubleshooting

| Symptom | Fix |
|---|---|
| _"Windows protected your PC"_ | Click **More info → Run anyway** |
| KCAN Dongle not detected | Open Device Manager — check for `Unknown device` under USB; reinstall WinUSB via Zadig if needed |
| Apex connect stalls after "booting" | The **bootloader** ID `0878 1101` isn't on WinUSB — bind it in Zadig (see the Apex section) |
| Apex not detected at all | Confirm the **application** ID `0878 1181` is bound to WinUSB in Zadig, then replug |
| Summit adapter not found | Verify `PCANBasic.dll` is installed: `where PCANBasic.dll` in CMD |
| App fails to start | Check Windows Event Viewer → Application log for the crash entry |
