#!/usr/bin/env bash
# sync-from-remote.sh — Pull sources back from a remote host to local Mac.
#
# Use this after fixing bugs directly on a remote Windows/Linux machine to
# bring those changes back to the primary Mac development machine.
#
# Usage:
#   ./tools/remote-dev/sync-from-remote.sh [HOST]
#
# HOST defaults to RUSTYCAN_REMOTE_HOST env var.
# If neither is set the script exits with an error — no default host is baked in.
# Does NOT use --delete so local-only files (configs, logs) are preserved.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HOST="${1:-${RUSTYCAN_REMOTE_HOST:-}}"
if [[ -z "${HOST}" ]]; then
  echo "error: no remote host specified."
  echo "  Usage: $0 <host-alias>  or  set RUSTYCAN_REMOTE_HOST env var"
  exit 1
fi
REMOTE_PATH="${RUSTYCAN_REMOTE_PATH:-code/RustyCAN}"

echo "← Pulling from ${HOST}:${REMOTE_PATH}"

# Show what will change before overwriting
rsync -avzn \
  --exclude "target/" \
  --exclude "*.jsonl" \
  --exclude "*.log" \
  --rsync-path="rsync" \
  "${HOST}:${REMOTE_PATH}/" \
  "${REPO_ROOT}/" \
  | grep -v "^receiving\|^sent\|^total\|speedup\|Transfer\|^\.$" || true

echo ""
read -r -p "Apply these changes? [y/N] " confirm
if [[ "${confirm}" != "y" && "${confirm}" != "Y" ]]; then
  echo "Aborted."
  exit 0
fi

rsync -avz \
  --exclude "target/" \
  --exclude "*.jsonl" \
  --exclude "*.log" \
  --rsync-path="rsync" \
  "${HOST}:${REMOTE_PATH}/" \
  "${REPO_ROOT}/"

echo "✓ Pull complete — run 'cargo build -p rustycan' to verify"
