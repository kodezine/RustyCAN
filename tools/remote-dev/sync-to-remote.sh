#!/usr/bin/env bash
# sync-to-remote.sh — Push local sources to a remote host for building/testing.
#
# Usage:
#   ./tools/remote-dev/sync-to-remote.sh [HOST]
#
# HOST defaults to the value of RUSTYCAN_REMOTE_HOST env var.
# If neither is set the script exits with an error — no default host is baked in.
# The remote path defaults to RUSTYCAN_REMOTE_PATH, then "code/RustyCAN".
#
# Prerequisites on the remote:
#   - OpenSSH Server running and reachable
#   - rsync in PATH (Windows: install via `choco install rsync`)
#   - Rust toolchain installed (`rustup install stable`)
#
# SSH config (~/.ssh/config) should have a Host entry for the remote, e.g.:
#   Host <your-alias>
#       HostName <REMOTE_IP>
#       User <REMOTE_USER>
#       IdentityFile ~/.ssh/id_rsa

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HOST="${1:-${RUSTYCAN_REMOTE_HOST:-}}"
if [[ -z "${HOST}" ]]; then
  echo "error: no remote host specified."
  echo "  Usage: $0 <host-alias>  or  set RUSTYCAN_REMOTE_HOST env var"
  exit 1
fi
REMOTE_PATH="${RUSTYCAN_REMOTE_PATH:-code/RustyCAN}"

echo "→ Syncing to ${HOST}:${REMOTE_PATH}"

rsync -avz --delete \
  --exclude "target/" \
  --exclude "*.jsonl" \
  --exclude "*.log" \
  --rsync-path="rsync" \
  "${REPO_ROOT}/" \
  "${HOST}:${REMOTE_PATH}/"

echo "✓ Sync complete"
