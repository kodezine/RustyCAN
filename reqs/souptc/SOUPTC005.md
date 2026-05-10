---
active: true
derived: false
level: 1.5
links:
- SOUP024: D7GsvauKhdYro4SFobL2t5gaSZMznwkM2_0u3F5byis=
method: automated
normative: true
ref: ''
reviewed: Ss8rNLFXQ00S-ki7Pf3PAO8frbazMjo5-IacSIaGfho=
test-command: 'cargo test -p rustycan -- eds::'
---

# EDS Parser Unit Tests

**Objective:** Verify internal EDS parsing logic — section classification, node-ID string parsing, and default value parsing.

**Test suite:** `host/src/eds/mod.rs` — `parse_section_var`, `parse_section_sub`, `parse_section_non_object`, `parse_node_id_str_variants`, `parse_default_u32_hex`, `parse_default_u32_decimal`, `build_entry_minimal` (8 tests).

**Pass criteria:** All 8 unit tests pass with exit code 0.

**Execution:**

```sh
cargo test -p rustycan -- eds::
```