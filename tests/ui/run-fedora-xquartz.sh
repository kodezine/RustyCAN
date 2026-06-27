#!/usr/bin/env bash
# run-fedora-xquartz.sh
#
# Launch the RustyCAN egui GUI on fedora-can with X11 forwarded to the local
# XQuartz instance.  The window appears on your macOS desktop.
#
# Prerequisites (macOS host):
#   - XQuartz installed and running (https://www.xquartz.org)
#   - `ssh -Y fedora-can` working (confirm: ssh -Y fedora-can 'echo $DISPLAY')
#
# Prerequisites (fedora-can):
#   - Repo cloned at ~/repo/RustyCAN
#   - `cargo build -p rustycan` succeeds
#   - can0 UP: sudo ip link set can0 up type can bitrate 250000
#
# Usage:
#   ./tests/ui/run-fedora-xquartz.sh [--screenshot] [--config <path>] [--dry-run]
#
# Options:
#   --screenshot   Capture a PNG of the GUI window after a short delay using
#                  scrot on the remote host.  File is copied back locally.
#   --config <p>   Path on the *remote* host to the config file.
#                  Defaults to: ~/repo/RustyCAN/host/config.linux.json
#   --dry-run      Print the SSH command without executing it.

set -euo pipefail

# ── Defaults ──────────────────────────────────────────────────────────────────
REMOTE_HOST="fedora-can"
REMOTE_REPO="~/repo/RustyCAN"
REMOTE_CONFIG="${REMOTE_REPO}/host/config.linux.json"
SCREENSHOT=0
DRY_RUN=0
SCREENSHOT_DELAY=3   # seconds to wait before capturing

# ── Argument parsing ──────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case "$1" in
        --screenshot)  SCREENSHOT=1; shift ;;
        --config)      REMOTE_CONFIG="$2"; shift 2 ;;
        --dry-run)     DRY_RUN=1; shift ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# ── SSH helper: always force a fresh connection so X11 forwarding is set up ──
# SSH ControlMaster multiplexing reuses an existing connection that may have
# been opened without -Y, meaning DISPLAY is never set on the remote side.
# -o ControlPath=none disables multiplexing for this call.
SSH_X11="ssh -Y -o ControlPath=none"

# ── Auto-detect DISPLAY for XQuartz if not already set ───────────────────────
# VS Code's integrated terminal (and many launchd-spawned shells) don't inherit
# the DISPLAY value that XQuartz exports.  Look for the well-known XQuartz
# socket (/private/tmp/.X11-unix/X*) and set DISPLAY accordingly.
if [[ -z "${DISPLAY:-}" ]]; then
    XQUARTZ_SOCKET=$(ls /private/tmp/.X11-unix/X* 2>/dev/null | head -1)
    if [[ -n "${XQUARTZ_SOCKET}" ]]; then
        DISPLAY=":${XQUARTZ_SOCKET##*X}"
        export DISPLAY
        echo "  Auto-set DISPLAY=${DISPLAY} (XQuartz socket detected)"
    fi
fi

# ── Pre-flight: verify XQuartz is listening ───────────────────────────────────
if [[ $DRY_RUN -eq 0 ]]; then
    if ! pgrep -q Xquartz 2>/dev/null && ! pgrep -q "X11.bin" 2>/dev/null; then
        echo "ERROR: XQuartz does not appear to be running."
        echo "       Start XQuartz (/Applications/Utilities/XQuartz.app) and retry."
        exit 1
    fi

    # Probe: confirm X11 forwarding actually sets DISPLAY on the remote side.
    REMOTE_DISPLAY=$(${SSH_X11} "${REMOTE_HOST}" 'echo $DISPLAY' 2>/dev/null)
    if [[ -z "${REMOTE_DISPLAY}" ]]; then
        echo "ERROR: X11 forwarding did not set DISPLAY on ${REMOTE_HOST}."
        echo "  Common fixes:"
        echo "    1. XQuartz > Preferences > Security > Allow connections from network clients"
        echo "    2. Check sshd: ssh ${REMOTE_HOST} 'grep -i X11 /etc/ssh/sshd_config'"
        echo "    3. Test manually: ${SSH_X11} ${REMOTE_HOST} 'echo \$DISPLAY'"
        exit 1
    fi
    echo "  DISPLAY: ${REMOTE_DISPLAY} (X11 forwarding OK)"
fi

# ── Build the remote command ──────────────────────────────────────────────────
# LIBGL_ALWAYS_SOFTWARE is NOT needed here — the window is forwarded to the
# local macOS GPU via XQuartz, so Mesa software GL is unnecessary.
# DISPLAY is exported explicitly: ControlPath=none opens a fresh SSH session
# and X11 forwarding sets $DISPLAY, but cargo may spawn sub-processes that
# don't inherit it unless it is explicitly in the command environment.
#
# WINIT_UNIX_BACKEND=x11  — forces winit to use the X11 event loop instead
#   of Wayland.  On Fedora the Wayland socket from the user's local graphical
#   session may still be visible inside the SSH session; winit would then
#   create a Wayland surface that renders correctly but silently discards all
#   forwarded X11 mouse/keyboard events.
# WAYLAND_DISPLAY=  — belt-and-suspenders: clear the socket name so winit
#   cannot accidentally open a Wayland connection.
REMOTE_CMD="export DISPLAY=\$DISPLAY WINIT_UNIX_BACKEND=x11 WAYLAND_DISPLAY=; cd ${REMOTE_REPO} && cargo run -p rustycan --bin rustycan -- --config ${REMOTE_CONFIG} --auto-connect"

if [[ $SCREENSHOT -eq 1 ]]; then
    TIMESTAMP="$(date +%Y%m%d_%H%M%S)"
    REMOTE_PNG="/tmp/rustycan_xquartz_${TIMESTAMP}.png"
    LOCAL_PNG="tests/ui/screenshots/rustycan_xquartz_${TIMESTAMP}.png"

    # scrot needs a moment for the window to fully render before grabbing it.
    # Run the GUI in the background, wait, screenshot, then kill cleanly.
    REMOTE_CMD="export DISPLAY=\$DISPLAY WINIT_UNIX_BACKEND=x11 WAYLAND_DISPLAY=; cd ${REMOTE_REPO} && \
cargo run -p rustycan --bin rustycan -- --config ${REMOTE_CONFIG} --auto-connect & \
RUSTYCAN_PID=\$! ; \
sleep ${SCREENSHOT_DELAY} ; \
DISPLAY=\$DISPLAY scrot --focused '${REMOTE_PNG}' 2>/dev/null \
  || scrot '${REMOTE_PNG}' ; \
kill \$RUSTYCAN_PID 2>/dev/null ; wait \$RUSTYCAN_PID 2>/dev/null ; true"

    mkdir -p tests/ui/screenshots
fi

# ── Print / execute ───────────────────────────────────────────────────────────
SSH_CMD="${SSH_X11} ${REMOTE_HOST} '${REMOTE_CMD}'"

if [[ $DRY_RUN -eq 1 ]]; then
    echo "Dry-run — would execute:"
    echo "  ${SSH_CMD}"
    exit 0
fi

echo "Connecting to ${REMOTE_HOST} with X11 forwarding…"
echo "  Config : ${REMOTE_CONFIG}"
if [[ $SCREENSHOT -eq 1 ]]; then
    echo "  Screenshot will be saved to: ${LOCAL_PNG}"
fi
echo ""

# shellcheck disable=SC2029
${SSH_X11} "${REMOTE_HOST}" "${REMOTE_CMD}"

# Retrieve screenshot if requested
if [[ $SCREENSHOT -eq 1 ]]; then
    echo ""
    echo "Fetching screenshot from ${REMOTE_HOST}:${REMOTE_PNG} …"
    scp "${REMOTE_HOST}:${REMOTE_PNG}" "${LOCAL_PNG}"
    echo "Saved: ${LOCAL_PNG}"
fi
