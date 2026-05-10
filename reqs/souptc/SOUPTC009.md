---
active: true
derived: false
level: 1.9
links:
- SOUP011: EfXu-k6_YQHZHz_piKPCOiJvLzva5wRIskTvGmom-jI=
method: automated
normative: true
ref: ''
reviewed: Gut2DCi6a5yU1FlTqDQhPCXJy99tlPwBvttv3Ijft6c=
test-command: 'cargo test -p rustycan -- dbc::'
---

# DBC Bit Extraction Unit Tests

**Objective:** Verify low-level DBC signal bit-extraction for Intel and Motorola byte orders, including cross-byte signals and two's complement conversion.

**Test suite:** `host/src/dbc/mod.rs` — `extract_intel_single_byte`, `extract_intel_nibble`, `extract_intel_cross_byte`, `extract_motorola_single_byte`, `to_signed_positive`, `to_signed_negative` (6 tests).

**Pass criteria:** All 6 unit tests pass with exit code 0.

**Execution:**

```sh
cargo test -p rustycan -- dbc::
```