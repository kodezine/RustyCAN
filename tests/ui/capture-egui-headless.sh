#!/usr/bin/env bash
# capture-egui-headless.sh — render the RustyCAN egui GUI on a headless
# Xvfb display and grab a PNG with scrot.  Intended to run ON the Linux host.
#
# Usage:
#   ./capture-egui-headless.sh <output.png> [extra rustycan args...]
#
# Env:
#   DELAY   seconds to wait for the GUI to render before the grab (default 7)
#   DISP    X display to use (default :99)
#   GEOM    Xvfb screen geometry (default 1400x900x24)
set -euo pipefail

OUT="${1:?usage: capture-egui-headless.sh <output.png> [rustycan args...]}"
shift || true

DELAY="${DELAY:-7}"
DISP="${DISP:-:99}"
GEOM="${GEOM:-1400x900x24}"
REPO="${HOME}/code/RustyCAN"
BIN="${REPO}/target/debug/rustycan"

export DISPLAY="${DISP}"

# Start a private Xvfb server.
Xvfb "${DISP}" -screen 0 "${GEOM}" >/tmp/xvfb-${DISP#:}.log 2>&1 &
XPID=$!
trap 'kill ${APP:-0} 2>/dev/null || true; kill ${XPID} 2>/dev/null || true' EXIT
sleep 1.5

cd "${REPO}"
# Force X11 backend; software GL (Mesa llvmpipe) since there is no GPU.
WINIT_UNIX_BACKEND=x11 WAYLAND_DISPLAY= LIBGL_ALWAYS_SOFTWARE=1 \
    "${BIN}" "$@" >/tmp/rustycan-gui.log 2>&1 &
APP=$!

sleep "${DELAY}"

# Grab the whole root window (no WM running, so the app owns the screen).
scrot --overwrite "${OUT}" 2>/dev/null || scrot "${OUT}"

# Clean shutdown.
kill "${APP}" 2>/dev/null || true
wait "${APP}" 2>/dev/null || true
APP=

echo "captured: ${OUT}"
ls -la "${OUT}"
