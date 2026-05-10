---
active: true
derived: false
level: 1.8
links:
- SOUP011: EfXu-k6_YQHZHz_piKPCOiJvLzva5wRIskTvGmom-jI=
method: automated
normative: true
ref: ''
reviewed: dC4TPNXFeercRfffX7gm8yirRuxFD0pUpGnCe8_HCEk=
test-command: cargo test -p rustycan -- dbc
---

# DBC Message and Signal Decoding (Integration)

**Objective:** Verify that DBC files are parsed and CAN signal values are correctly decoded including scaling, offset, and VAL_ descriptions.

**Test suite:** `host/tests/integration_test.rs` — `dbc_parse_fixture_message_name`, `dbc_decode_engine_speed_intel_le`, `dbc_decode_coolant_temp_with_val_description` (3 tests).

**Pass criteria:** `EngineData` message parses correctly from `tests/fixtures/sample_bus.dbc`; `EngineSpeed` raw value `0x0320` decodes to 100.0 rpm; `CoolantTemp` decodes to expected VAL_ description.

**Execution:**

```sh
cargo test -p rustycan -- dbc
```