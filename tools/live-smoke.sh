#!/usr/bin/env bash
#
# live-smoke.sh — real-bus smoke test for RustyCAN.
#
# Opens the Summit adapter (listen-only by default), captures decoded CAN events
# for a fixed window, and asserts that the basic pipeline is alive:
#   * adapter opens and receives frames
#   * NMT and PDO decoders produce output
#
# Re-run this before and after every change during the egui-0.35 / sniffer work
# to prove the basic functions still work against the real CANopen bus.
#
# Usage:
#   tools/live-smoke.sh [CONFIG] [DURATION_SECONDS]
#
# Env:
#   MIN_FRAMES   minimum total decoded frames required (default 20)
#   REQUIRE_SDO  set to 1 to also require at least one SDO event (default 0)
#
# Exit code 0 = pass, non-zero = fail.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

CONFIG="${1:-host/config.smoke-summit-250k.json}"
DURATION="${2:-8}"
MIN_FRAMES="${MIN_FRAMES:-20}"
REQUIRE_SDO="${REQUIRE_SDO:-0}"

if [[ ! -f "$CONFIG" ]]; then
  echo "FAIL: config not found: $CONFIG" >&2
  exit 2
fi

echo "── live-smoke ─────────────────────────────────────────────"
echo "config    : $CONFIG"
echo "duration  : ${DURATION}s"
echo "min frames: $MIN_FRAMES"
echo

echo "Building rustycan…"
cargo build -p rustycan --bin rustycan >/dev/null

TMP="$(mktemp -t rustycan-smoke.XXXXXX)"
trap 'rm -f "$TMP"' EXIT

echo "Capturing ${DURATION}s from the bus…"
"$ROOT/target/debug/rustycan" --config "$CONFIG" --log-to-stdout >"$TMP" 2>&1 &
PID=$!
sleep "$DURATION"
kill -INT "$PID" 2>/dev/null || true
wait "$PID" 2>/dev/null || true

# Decoded stdout lines look like:
#   [12:25:39.346] NMT    node  30  state OPERATIONAL
#   [12:25:39.513] PDO    node 117  cob 0x1F5  Byte0 = [01]
#   [12:25:40.935] SDO    node  30  READ   3204:01 …
nmt=$(grep -cE '\] NMT ' "$TMP" || true)
pdo=$(grep -cE '\] PDO ' "$TMP" || true)
sdo=$(grep -cE '\] SDO ' "$TMP" || true)
total=$(( nmt + pdo + sdo ))

echo
echo "── results ────────────────────────────────────────────────"
echo "NMT : $nmt"
echo "PDO : $pdo"
echo "SDO : $sdo"
echo "TOTAL: $total"
echo

fail=0
if (( total < MIN_FRAMES )); then
  echo "FAIL: only $total frames (< $MIN_FRAMES). Adapter/bus problem?" >&2
  fail=1
fi
if (( nmt < 1 )); then
  echo "FAIL: no NMT frames decoded." >&2
  fail=1
fi
if (( pdo < 1 )); then
  echo "FAIL: no PDO frames decoded." >&2
  fail=1
fi
if [[ "$REQUIRE_SDO" == "1" ]] && (( sdo < 1 )); then
  echo "FAIL: no SDO frames decoded (REQUIRE_SDO=1)." >&2
  fail=1
fi

if (( fail == 0 )); then
  echo "PASS ✅  basic RX + CANopen decode working."
else
  echo "── first 20 captured lines (for triage) ───────────────────" >&2
  head -20 "$TMP" >&2
fi
exit "$fail"
