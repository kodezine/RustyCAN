---
active: true
derived: false
level: 1.1
links:
- SOUP007: ji-5u2RzHuCwzihEuq4KEiVzAQXNlzSUS4BTeELb63Y=
method: automated
normative: true
ref: ''
reviewed: i4Ec55oMm-0Y0fK8CVlSReocxvAF-8iajTKr4j0Gse8=
test-command: cargo test -p rustycan -- nmt
---

# NMT Heartbeat Decode

**Objective:** Verify that NMT heartbeat frames are correctly decoded to node state values.

**Test suite:** `host/src/canopen/nmt.rs` — unit tests `decode_bootup`, `decode_operational`, `decode_pre_op`, `decode_start_all`, `returns_none_on_empty` (6 tests).

**Pass criteria:** All tests pass with exit code 0.

**Execution:**

```sh
cargo test -p rustycan -- nmt
```