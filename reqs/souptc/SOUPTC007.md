---
active: true
derived: false
level: 1.7
links:
- SOUP005: NpondUAhEdUiTc_75hqMVN1xBiIrHFqNUqQ0xur_j1c=
method: automated
normative: true
ref: ''
reviewed: rnxfbDdkexxlIWfpuRId6aH15c5Iqd_1jmTXvk5yGwk=
test-command: cargo test -p rustycan -- classify
---

# CAN Frame COB-ID Classification

**Objective:** Verify that incoming CAN frame COB-IDs are correctly classified as heartbeat, TPDO, or RPDO frame types.

**Test suite:** `host/tests/integration_test.rs` — `classify_heartbeat_frame`, `classify_tpdo_rpdo`.

**Pass criteria:** COB-IDs 0x701–0x702 are classified as heartbeat; TPDO and RPDO COB-ID ranges are correctly distinguished.

**Execution:**

```sh
cargo test -p rustycan -- classify
```