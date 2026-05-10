---
active: true
derived: false
level: 3.1
links:
- SOUP022: i-gGH4UqQmt1MPhvGxkCBPc25B12qLwmGUCeYY9u6Nc=
- SOUP023: Dobe1n9Xq81CkgTv5_OlWo-_c40cqZUzS61W_8oLlpo=
method: review
normative: true
ref: ''
reviewed: NX33AYJCLPDxiSUzPVl8gl_ICiEFP7lxIl8si9LzWXA=
test-command: ''
---

# Code Review: Adapter Interface Implementation

**Objective:** Confirm by code review that `host/src/adapters/peak.rs` and `host/src/adapters/kcan.rs` correctly implement the declared hardware interface requirements.

**Reviewers:** At least one reviewer with USB and CAN bus expertise.

**Review checklist:**

- `peak.rs`: Uses vendor PEAK driver API on macOS/Windows; uses SocketCAN `AF_CAN` socket on Linux. Correct error propagation on driver failure.
- `kcan.rs`: Uses `nusb` exclusively (no `libusb` or `rusb` imports). Bulk endpoints OUT=0x01, IN=0x81. Call order: `open()` → `claim_interface(0)` → control/bulk I/O. GET_INFO retry logic present. Bulk OUT chunked to 64-byte MPS.

**Pass criteria:** Review completed and documented. No deviations from SOUP023 (nusb-only) or SOUP022 (driver mapping) found, or deviations are recorded as new anomaly items.