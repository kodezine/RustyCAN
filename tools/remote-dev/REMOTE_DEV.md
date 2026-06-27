# RustyCAN Remote Development Guide

Cross-platform UI testing uses a Mac as the primary dev machine, with Windows
and/or Linux remotes for build verification and screenshot comparison.

## Prerequisites

### Mac (source machine)
- SSH key at `~/.ssh/id_rsa` with public key copied to all remotes
- `~/.ssh/config` entry per remote (see below)

### Windows remote
- OpenSSH Server running (`Get-Service sshd` → Running)
- Rust stable: `rustup install stable`
- rsync: `choco install rsync` (requires Chocolatey)
- PATH: `~/.cargo/bin` and `C:\ProgramData\chocolatey\bin` in `~/.bashrc`
- PEAK driver: install from https://www.peak-system.com/PCAN-Basic.239.0.html
- Config: `host/config.windows.json` (committed, edit `nodes` as needed)

### Linux remote (headless Fedora)

**One-time setup on Fedora (as root/sudo):**
```bash
# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source ~/.cargo/env

# Build dependencies
dnf install -y gcc pkg-config

# Software OpenGL (Mesa llvmpipe) — for on-demand GUI via X11 forwarding
dnf install -y mesa-libGL mesa-dri-drivers libX11 libXcursor libXrandr libXi

# Enable X11 forwarding in sshd
echo "X11Forwarding yes" >> /etc/ssh/sshd_config
systemctl restart sshd

# udev rule for KCAN USB dongle (replug after adding)
echo 'SUBSYSTEM=="usb", ATTR{idVendor}=="cafe", ATTR{idProduct}=="beef", MODE="0666"' \
  > /etc/udev/rules.d/99-kcan.rules
udevadm control --reload-rules
```

### Linux remote (Ubuntu/Debian)

**One-time setup on Ubuntu (tested: 26.04 LTS "resolute"):**
```bash
# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path
echo 'source ~/.cargo/env' >> ~/.bashrc
source ~/.cargo/env

# Build + OpenGL + X11 + Xvfb + capture tools
sudo apt-get update -qq && sudo apt-get install -y \
  build-essential pkg-config libssl-dev \
  libgl1-mesa-dri libegl1 mesa-utils \
  libx11-dev libxcursor-dev libxrandr-dev libxi-dev libxext-dev \
  xvfb scrot xauth \
  rsync curl

# Enable X11 forwarding in sshd
sudo sed -i 's/^#*X11Forwarding.*/X11Forwarding yes/' /etc/ssh/sshd_config
sudo systemctl reload ssh

# udev rule for KCAN USB dongle (replug after adding)
echo 'SUBSYSTEM=="usb", ATTR{idVendor}=="cafe", ATTR{idProduct}=="beef", MODE="0666"' | \
  sudo tee /etc/udev/rules.d/99-kcan.rules > /dev/null
sudo udevadm control --reload-rules

# Create repo directory
mkdir -p ~/code/RustyCAN
```

Config: `host/config.linux.json` — uses `KCanDongle` adapter (PCAN not supported on Linux).

## SSH Config (~/.ssh/config)

```
Host <windows-host-alias>
    HostName <WINDOWS_IP>
    User <WINDOWS_USERNAME>
    IdentityFile ~/.ssh/id_rsa

# Linux headless Fedora — X11 forwarding enabled for on-demand GUI
Host <linux-host-alias>
    HostName <LINUX_IP>
    User <LINUX_USERNAME>
    IdentityFile ~/.ssh/id_rsa
    ForwardX11 yes
    ForwardX11Trusted yes
```

## Workflow

### Push changes to remote and build

```bash
# Sync sources (excludes target/, *.jsonl, *.log)
./tools/remote-dev/sync-to-remote.sh <windows-host-alias>

# Build on remote
ssh <windows-host-alias> "cd code/RustyCAN && cargo build -p rustycan 2>&1"

# Or both in one step via VS Code task:
#   Ctrl+Shift+P → "Tasks: Run Task" → "remote: build on <windows-host-alias> (Windows)"
```

### Pull bug fixes from remote back to Mac

```bash
./tools/remote-dev/sync-from-remote.sh <windows-host-alias>
# Shows a dry-run diff, asks for confirmation before overwriting
```

### Run the app on remote (Linux — headless)

```bash
# ── Lightweight TUI over plain SSH (no display needed, works on old hardware) ──
ssh fedora-can "cd code/RustyCAN && cargo run -p rustycan --bin rustycan -- \
  --tui --config host/config.linux.json 2>&1"

# ── On-demand GUI via X11 forwarding (renders on Mac, not on the server) ──
# Prerequisites on Mac: install XQuartz (https://www.xquartz.org), log out/in once.
# Then:
ssh -X fedora-can "cd code/RustyCAN && \
  LIBGL_ALWAYS_SOFTWARE=1 \
  cargo run -p rustycan --bin rustycan -- --config host/config.linux.json 2>&1"
# The GUI window opens on your Mac. The Fedora server only sends draw commands.
```

Or open the folder in VS Code Remote SSH and press F5
using the **RustyCAN (Windows)** launch configuration.

## VS Code Remote SSH

1. Install **Remote - SSH** extension on Mac
2. `Cmd+Shift+P` → `Remote-SSH: Connect to Host` → `<windows-host-alias>`
3. Open folder `C:\Users\<WINDOWS_USERNAME>\code\RustyCAN`
4. Install **rust-analyzer** and **GitHub Copilot** on the remote when prompted
5. F5 → **RustyCAN (Windows)** to debug

## Platform-Specific Notes

| Feature              | macOS                         | Windows                        |
|----------------------|-------------------------------|--------------------------------|
| PEAK library         | `libPCBUSB.dylib` (mac-can)   | `PCANBasic.dll` (PEAK official)|
| RTLD_NODELETE guard  | `#[cfg(target_os = "macos")]` | skipped (not needed)           |
| Log path fallback    | project dir                   | `%USERPROFILE%` if ACL blocked |
| Adapter detection    | `ioreg` / nusb                | nusb only                      |
| stderr redirect      | `#[cfg(unix)]`                | no-op                          |
