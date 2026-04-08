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
is included in the mainline kernel (4.1+). Load it if not already loaded:

```sh
sudo modprobe peak_usb
```

Verify the interface appeared:

```sh
ip link show | grep can
# Expected: can0: <NOARP,ECHO> ...
```

Bring it up at the desired baud rate:

```sh
sudo ip link set can0 type can bitrate 250000
sudo ip link set can0 up
```

RustyCAN uses SocketCAN via the `socketcan` feature of the `host-can` crate —
no proprietary library is required.

---

## First launch

1. Launch `rustycan` from the terminal, app launcher (.deb), or AppImage.
2. On the **Connect** screen, choose your adapter.
3. Optionally browse to one or more `.eds` files for connected nodes.
4. Click **Connect**.

See the [README](../README.md#using-the-gui) for a full GUI walkthrough.
