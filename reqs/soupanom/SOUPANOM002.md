---
active: true
derived: false
iec62304-clause: 8.1.3
level: 1.2
links:
- SOUP003: srULBWyD6Am61fmZzwHaKSHX8wRo7LpLu75pI0wKgrA=
normative: true
ref: ''
reviewed: w7HhzBbgHbBcpSljxZwKt2jhO1dZfRnmIRR2l23cA70=
type: anomaly
---

# macOS: GET_INFO Returns Cancelled on First Attempt

**Affected platforms:** macOS

**Description:** After `claim_interface(0)`, the first `GET_INFO` control request to the KCAN Dongle may return a `Cancelled` error (`kIOReturnAborted`) due to macOS IOKit initialisation latency.

**Workaround:** The application retries the `GET_INFO` request up to five (5) times with 50 ms exponential backoff before reporting a connection failure.

**Impact:** Connection setup may take up to ~250 ms longer on macOS on first attach. No data loss occurs.