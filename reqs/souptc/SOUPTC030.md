---
active: true
derived: false
doc:
  copyright: RustyCAN
  title: RustyCAN — SOUP Test Cases
level: 3.6
links:
- SOUP029: 55WtEcGqmycrGWfRg5Jp7qaDXW9Ap5FdKUfxYc9qE2o=
method: automated
normative: true
ref: ''
reviewed: ltAAdFY2D_DEOk49b4XRueDbuKQxCw2FK74AyhKFkJM=
test-command: cargo test -p rustycan -- xcp::a2l
---

# A2L Parser

**Objective:** Verify A2L MEASUREMENT/CHARACTERISTIC parsing and raw value decode.

**Test suite:** `host/src/xcp/a2l.rs` unit tests.

**Pass criteria:** All tests pass with exit code 0.