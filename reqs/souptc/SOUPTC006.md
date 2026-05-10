---
active: true
derived: false
level: 1.6
links:
- SOUP010: SXfqm84Qq2a6KjUF_TwlUpwnw6x6ud2Ga1WRQH-eQA0=
method: automated
normative: true
ref: ''
reviewed: qu6O1msADWoUM3nTg57RdCAehYDiEFGPUAC3_NEGUJ8=
test-command: cargo test -p rustycan -- sdo
---

# SDO Expedited and Segmented Transfer

**Objective:** Verify that SDO command frames are correctly encoded and decoded for expedited and segmented upload/download operations.

**Test suite:** `host/tests/integration_test.rs` — `sdo_decodes_status_word_with_eds`, `sdo_encode_upload_request_roundtrip`, `sdo_encode_download_expedited_u16`, `sdo_encode_value_u32_known_object`, `sdo_parse_hex_bytes_and_segmented_init` (4–5 tests).

**Pass criteria:** All SDO integration tests pass; round-trip encode/decode produces identical frames; EDS-looked-up object names and values match expectations.

**Execution:**

```sh
cargo test -p rustycan -- sdo
```