---
active: true
derived: false
level: 1.2
links:
- SOUP008: sObYhdCaHSchNdbsuBNtW2THIf7vRY0ETuLSgUvx5r4=
method: automated
normative: true
ref: ''
reviewed: Qtpmqnoevr_hI1Bgmkb-txgrIQB3ikol-PWaw7KB4OY=
test-command: cargo test -p rustycan -- nmt::tests::encode_all_commands
---

# NMT Master Command Encoding

**Objective:** Verify that NMT master commands are correctly encoded into CAN frames.

**Test suite:** `host/src/canopen/nmt.rs` — unit test `encode_all_commands`.

**Pass criteria:** All NMT command variants (Start, Stop, EnterPreOperational, ResetNode, ResetCommunication) encode to the expected COB-ID 0x000 frame bytes.

**Execution:**

```sh
cargo test -p rustycan -- nmt::tests::encode_all_commands
```