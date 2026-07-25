---
active: true
derived: false
doc:
  copyright: RustyCAN
  title: RustyCAN — SOUP Test Cases
level: 3.5
links:
- SOUP028: m8ZsxhlNNohAlbVRHP2jpUW6sH_sNrmc6-r83M-dk_g=
method: automated
normative: true
ref: ''
reviewed: MhkGQuoXE4EPyNpI_Z6wTDMBEiK-qADQf9NZzcEjqjw=
test-command: cargo test -p rustycan -- xcp::daq
---

# XCP DAQ Tracker

**Objective:** Verify dynamic DAQ configuration tracking and DTO DAQ frame decode.

**Test suite:** `host/src/xcp/daq.rs` unit tests.

**Pass criteria:** All tests pass with exit code 0.