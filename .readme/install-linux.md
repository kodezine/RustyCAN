# Installing RustyCAN on Linux

> **Platform:** x86-64 · glibc 2.35+ (Ubuntu 22.04 LTS, Debian 12, Fedora 36+, Arch)

## Option A — AppImage (any distro, no install required)

AppImage runs on any x86-64 Linux without installation.

```sh
# 1. Download
wget https://github.com/kodezine/RustyCAN/releases/latest/download/rustycan-<version>-x86_64-linux.AppImage

# 2. Make executable
chmod +x rustycan-*.AppImage

# 3. Run
./rustycan-*.AppImage
```

> **FUSE required:** AppImage needs `libfuse2`. Install if missing:
> ```sh
> # Ubuntu / Debian
> sudo apt install libfuse2
> # Fedora
> sudo dnf install fuse-libs
> # Arch
> sudo pacman -S fuse2
> ```

---

## Option B — Debian / Ubuntu `.deb` package

```sh
# 1. Download
wget https://github.com/kodezine/RustyCAN/releases/latest/download/rustycan_<version>_amd64.deb

# 2. Install
sudo dpkg -i rustycan_*.deb

# 3. Launch
rustycan
```

A `.desktop` entry is created automatically — RustyCAN appears in your app launcher.

### Uninstall

```sh
sudo dpkg -r rustycan
```

---

## Prerequisites

### USB access — udev rules

Linux restricts raw USB access to root by default. Install the provided udev
rules so RustyCAN can open USB devices as a normal user:

```sh
sudo cp /opt/RustyCAN/packaging/50-rustycan.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger
```

Or download the rules file directly from the repository:

```sh
sudo curl -fsSL \
  https://raw.githubusercontent.com/kodezine/RustyCAN/main/host/packaging/50-rustycan.rules \
  -o /etc/udev/rules.d/50-rustycan.rules
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Then **re-plug** your USB adapter.

The rules grant write access to:
| Device | VID | PID |
|---|---|---|
| KCAN Dongle | `0x1209` | `0xBEEF` |
| PEAK PCAN-USB | `0x0c72` | all PIDs |

### KCAN Dongle

No additional drivers are needed beyond the udev rules above.

### PEAK PCAN-USB adapter (optional)

PEAK adapters on Linux use the **SocketCAN** kernel driver (`peak_usb`), which
is included in the mainline kernel (4.1+). No proprietary library is needed.

#### Step 1 — Load the kernel module (once per boot)

```sh
sudo modprobe peak_usb
```

To load it automatically at boot:

```sh
echo 'peak_usb' | sudo tee /etc/modules-load.d/peak_usb.conf
```

#### Step 2 — Plug in the adapter

After plugging in, verify the interface appeared:

```sh
ip link show | grep can
# Expected output: can0: <NOARP,ECHO> mtu 16 ...
```

> If nothing appears, the module may not be loaded or the adapter needs to be
> re-plugged after loading.

#### Step 3 — Bring up the interface

```sh
sudo ip link set can0 up type can bitrate 250000
```

Verify it is UP and the bitrate is set:

```sh
ip -details link show can0
# Expected: state UP ... bitrate 250000
```

To make the bring-up persistent across reboots, create a `systemd-networkd`
configuration or a `udev` rule — see your distro’s documentation.

#### What happens if you skip these steps?

RustyCAN checks for common problems before opening the socket and shows clear
step-by-step guidance in the Connect screen error banner if:

| Problem | Message shown |
|---------|---------------|
| `peak_usb` module not loaded | Step-by-step: modprobe + bring up |
| Module loaded, adapter not plugged in | Lists available CAN interfaces |
| Interface exists but is DOWN | Exact `ip link set` command to run |
| Interface name is wrong | Lists available CAN interfaces |

RustyCAN uses the standard Linux `AF_CAN` / `PF_CAN` socket API via the
`socketcan` crate — no proprietary library is required.

---

## First launch

1. Launch `rustycan` from the terminal, app launcher (.deb), or AppImage.
2. On the **Connect** screen, select **SocketCAN** (Linux-only radio button).
3. Set the **Interface** field to your CAN interface name (default: `can0`).
4. Optionally browse to one or more `.eds` files for connected nodes.
5. Click **Connect**.

> **KCAN Dongle users:** select **KCAN Dongle ★** instead of SocketCAN.
> No additional kernel module is required.

See the [GUI Guide](gui-guide.md) for a full GUI walkthrough.
