---
active: true
derived: false
iec62304-clause: 8.1.3
level: 1.1
links:
- SOUP003: srULBWyD6Am61fmZzwHaKSHX8wRo7LpLu75pI0wKgrA=
- SOUP023: Dobe1n9Xq81CkgTv5_OlWo-_c40cqZUzS61W_8oLlpo=
normative: true
ref: ''
reviewed: hLinqynnG7oj1LHWeueKGdXqZ3fCemAByzwmt5JZ9MI=
type: anomaly
---

# macOS: set_configuration() Causes kIOReturnAborted

**Affected platforms:** macOS

**Description:** On macOS, calling `set_configuration()` on the USB device after `claim_interface(0)` causes all subsequent control-endpoint (EP0) requests to fail with `kIOReturnAborted`. This prevents `GET_INFO` and other KCAN control commands from executing.

**Workaround:** The application omits `set_configuration()` entirely. The call order used is: `open()` → `claim_interface(0)` → `control_in/out()`. No `set_configuration()` or `set_alt_setting()` call is made.

**Impact:** None at runtime; the device operates correctly without explicit configuration selection because the KCAN Dongle presents a single configuration.