---
active: true
derived: false
doc:
  copyright: RustyCAN
  title: RustyCAN — SOUP Test Cases
level: 3.4
links:
- SOUP027: 3dRCZxWzcj2bKOqmWlok0VgIIkF5kyK246X_Mv54zo0=
method: automated
normative: true
ref: ''
reviewed: DkDhq2Q6SBYsm8q3z4eomudDyWRgGVqk_23El_yLayk=
test-command: cargo test -p rustycan -- xcp::command
---

# XCP Command Codec

**Objective:** Verify encoding of XCP master commands and decoding of CONNECT and error responses.

**Test suite:** `host/src/xcp/command.rs` unit tests.

**Pass criteria:** All tests pass with exit code 0.