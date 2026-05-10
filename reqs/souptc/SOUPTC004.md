---
active: true
derived: false
level: 1.4
links:
- SOUP009: hurj0RcHs-cEv4a3q4lq6r71aCqvHncacvUefonzymE=
method: automated
normative: true
ref: ''
reviewed: di4DtLDODWR_zjyHh94-km4Z3SktoQmsBrNR2_zC0IY=
test-command: cargo test -p rustycan -- eds_parse
---

# EDS File Parsing (Integration)

**Objective:** Verify that EDS files are correctly parsed into an object dictionary with the expected entries.

**Test suite:** `host/tests/integration_test.rs` — `eds_parse_device_type`, `eds_parse_status_word`, `eds_parse_tpdo1_sub_objects` (3 tests).

**Pass criteria:** Device type (0x1000), Status Word (0x3000), and TPDO1 mapping (0x1A00 sub-objects) parse to the expected types, names, and default values from `tests/fixtures/sample_drive.eds`.

**Execution:**

```sh
cargo test -p rustycan -- eds_parse
```