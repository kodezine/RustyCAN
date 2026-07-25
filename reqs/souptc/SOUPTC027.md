---
active: true
derived: false
doc:
  copyright: RustyCAN
  title: RustyCAN — SOUP Test Cases
level: 3.3
links:
- SOUP026: E-LKY-T3qu98zKerEHkr6w_p90HrWFrrt1TYfhhVKfs=
method: automated
normative: true
ref: ''
reviewed: dj1GqVu_WU2AfAqdDrn5HjubbB3ecCNtPZg0ZCIRkzM=
test-command: cargo test -p rustycan -- xcp::tests
---

# XCP Transport Routing Decode

**Objective:** Verify XCP CRO/DTO classification and slave byte-order handling.

**Test suite:** `host/src/xcp/mod.rs` unit tests.

**Pass criteria:** All tests pass with exit code 0.