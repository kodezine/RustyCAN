# 🍎 Installing RustyCAN on macOS

[![Release](https://github.com/kodezine/RustyCAN/actions/workflows/release.yml/badge.svg)](https://github.com/kodezine/RustyCAN/actions/workflows/release.yml)

> **Platform:** macOS 12 Monterey or later · Apple Silicon (arm64) and Intel (x86-64)

---

## Option A — Homebrew (recommended) ✅

The easiest way. Homebrew handles download, quarantine removal, and future upgrades.

```sh
brew tap kodezine/rustycan
brew install --cask rustycan
```

> ℹ️ RustyCAN is not notarized (no Apple Developer subscription). The cask
> automatically strips the Gatekeeper quarantine flag so the app opens without
> any "damaged" warning.

| Step | Command | Expected result |
|---|---|---|
| ✅ Tap added | `brew tap kodezine/rustycan` | `Tapped 1 cask` |
| ✅ App installed | `brew install --cask rustycan` | `RustyCAN.app` in `/Applications` |
| ✅ No quarantine | _(automatic via cask preflight)_ | App opens without Gatekeeper popup |

### Upgrade

```sh
brew upgrade --cask rustycan
```

### Uninstall

```sh
brew uninstall --cask rustycan
brew untap kodezine/rustycan
```

---

## Option B — Direct DMG download

1. Go to the [⬇️ Releases page](https://github.com/kodezine/RustyCAN/releases).
2. Download **`rustycan-<version>-aarch64-apple-darwin.dmg`** (Apple Silicon)  
   or the `x86_64` variant for Intel Macs.
3. Open the DMG and drag **RustyCAN.app** to `/Applications`.

   ![macOS drag-to-install](https://docs.github.com/assets/cb-25535/mw-1440/images/help/repository/releases-tab.webp)
   *(open the DMG → drag RustyCAN.app to the Applications shortcut)*

4. ⚠️ Remove the quarantine flag that macOS adds to downloaded apps:

   ```sh
   xattr -dr com.apple.quarantine /Applications/RustyCAN.app
   ```

5. ✅ Launch from Spotlight (`⌘ Space` → `RustyCAN`) or Finder.

> **Why step 4?** macOS marks apps downloaded outside the Mac App Store as
> "quarantined". Without a $99/year Apple Developer certificate the app cannot
> be notarized, so Gatekeeper shows _"RustyCAN is damaged and can't be opened"_.
> The `xattr` command removes that flag permanently.

---

## 🔌 Prerequisites

### KCAN Dongle — ✅ no extra drivers needed

The KCAN Dongle (VID `0x1209` / PID `0xBEEF`) enumerates as a standard USB
bulk device. macOS loads the generic USB driver automatically.

### PEAK PCAN-USB adapter — optional

If you want to use a PEAK PCAN-USB adapter:

| Step | Action |
|---|---|
| 1️⃣ | Download the latest **PCUSB** `.pkg` from <https://mac-can.com> |
| 2️⃣ | Run the installer — places `libPCBUSB.dylib` in `/usr/local/lib/` |
| 3️⃣ | Connect the PEAK adapter (appears as channel `1` by default) |
| 4️⃣ | Launch RustyCAN → select **PEAK PCAN-USB** on the Connect screen |

> ℹ️ If the library is missing, RustyCAN shows a friendly message with the
> download URL — the app still starts and the KCAN Dongle path is unaffected.

---

## 🚀 First launch

1. Open **RustyCAN** from `/Applications` or Spotlight (`⌘ Space` → `RustyCAN`).
2. On the **Connect** screen:
   - Choose **KCAN Dongle** or **PEAK PCAN-USB**
   - Set baud rate (default `250000`)
   - Optionally browse to `.eds` files for your nodes
3. Click **Connect** — the button activates automatically when the adapter is detected.

See the [GUI Guide](gui-guide.md) for a full GUI walkthrough.

---

## 🛠 Troubleshooting

| Symptom | Fix |
|---|---|
| _"RustyCAN is damaged and can't be opened"_ | `xattr -dr com.apple.quarantine /Applications/RustyCAN.app` |
| Connect button stays grey (KCAN) | Re-plug dongle; check `system_profiler SPUSBDataType \| grep -A5 KCAN` |
| Connect button stays grey (PEAK) | Verify `libPCBUSB.dylib` exists: `ls /usr/local/lib/libPCBUSB.dylib` |
| App crashes on launch | Check Console.app for crash report; file an issue with the log |
