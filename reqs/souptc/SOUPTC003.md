---
active: true
derived: false
level: 1.3
links:
- SOUP009: hurj0RcHs-cEv4a3q4lq6r71aCqvHncacvUefonzymE=
method: automated
normative: true
ref: ''
reviewed: gung2s4s32sQDM9J-9HIwh02HmvVhQzFZlAbhGq5AZg=
test-command: cargo test -p rustycan -- pdo
---

# PDO Decoding from EDS

**Objective:** Verify that a PDO frame is correctly decoded into named signal values using mappings from an EDS file.

**Test suite:** `host/tests/integration_test.rs` — `pdo_decoder_builds_from_eds`.

**Pass criteria:** A known 4-byte PDO payload is decoded to the expected Status Bits, Digital Inputs, and Current Segment values as defined in `tests/fixtures/sample_drive.eds`.

**Execution:**

```sh
cargo test -p rustycan -- pdo
```