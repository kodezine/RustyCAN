---
active: true
derived: false
iec62304-clause: 8.1.3
level: 1.6
links:
- SOUP003: srULBWyD6Am61fmZzwHaKSHX8wRo7LpLu75pI0wKgrA=
normative: true
ref: ''
reviewed: BWUyyx6nZNjYJ5PUbbdL0vryP-nvAC1wIQOHiDrBxs4=
type: anomaly
---

# KCAN Bulk IN Timeout Must Exceed Frame Interval

**Affected platforms:** All

**Description:** If the bulk IN read timeout is shorter than the firmware's frame transmission interval, the host receives spurious timeout errors that are indistinguishable from a genuine adapter stall. For a 100 ms echo/heartbeat interval the minimum safe timeout is 200 ms.

**Workaround:** The application sets the bulk IN read timeout to at least 200 ms (2× the expected maximum inter-frame interval). This value must be reviewed if the firmware frame interval changes.

**Impact:** Setting an insufficient timeout causes false `AdapterDisconnected` events. Setting an excessively large timeout delays stall detection. The current 200 ms value is a documented configuration constant.