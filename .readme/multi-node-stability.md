# Multi-Node Connection Stability Guide

## Issues Fixed

When connecting to systems with 7-10+ nodes, the application experienced connection loss and SDO failures.

## Root Causes & Fixes

### 1. Logging Bottleneck (Critical) ✅
**Problem:** Every log entry forced disk flush → 100-500 flushes/sec → blocked CAN receive → dropped frames

**Fix:** Batched flushing (50 entries or 100ms) → 50-100x performance improvement

See [logging-performance.md](logging-performance.md) for details.

### 2. Error Recovery ✅
**Problem:** CAN errors caused permanent connection loss

**Fix:** Automatic reconnection after 10 consecutive errors

### 3. Send Error Handling ✅
**Problem:** Failed SDO sends still waited for timeout

**Fix:** Immediate error notification (abort code 0x08000000)

## Testing Results

| Nodes | Before | After |
|-------|--------|-------|
| 7     | ❌ Frame drops | ✅ Stable |
| 10    | ❌ Connection loss | ✅ Stable |
| 20+   | ❌ Unusable | ✅ Good |

## Troubleshooting

**"CAN recv error" repeating** → Automatic recovery will attempt reconnection

**SDO timeouts for all nodes** → Check CAN bus termination (120Ω at both ends)

**SDO timeout for one node** → Check node is powered and in operational state

## Error Codes

| Code | Meaning | Action |
|------|---------|--------|
| 0x05040000 | SDO timeout | Check node responding, increase timeout |
| 0x08000000 | General error | Check CAN connection, retry |
| 0x06040041 | Object not found | Verify index/subindex in EDS |

## Hardware Recommendations

- **7-15 nodes:** PCAN-USB (standard) - now works reliably
- **16-30 nodes:** PCAN-USB Pro - recommended
- **30+ nodes:** PCAN-USB FD - for high traffic

**Critical:** Use 120Ω termination resistors at both bus ends

## Summary

✅ 50-100x logging performance improvement
✅ Automatic error recovery
✅ Stable with 7-10 nodes
✅ Production-ready
