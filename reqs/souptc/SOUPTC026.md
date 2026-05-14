---
active: true
derived: false
level: 2.15
links:
- SOUP025: l_BpydkvPLEDKrRVfYVRmmCcZTsJ58j8FFbRXs3_62o=
method: automated
normative: true
ref: ''
reviewed: qpFqOi-QqYpi4SrqWBMfHv90K5hfMA2h2WNn02FfRyU=
test-command: cargo test -p rustycan -- updater::tests
---

# App Update: Version Tag Parsing

**Objective:** Verify that `updater::parse_semver_tag` correctly parses Git version tags into `(major, minor, patch)` triples and rejects malformed input.

**Test suite:** `host/src/updater.rs` — unit tests in `updater::tests`:

| Test name | Input | Expected output |
|---|---|---|
| `parse_plain_tag` | `"v1.2.3"` | `Some((1, 2, 3))` |
| `parse_describe_suffix` | `"v0.2.0-5-gabcdef"` | `Some((0, 2, 0))` |
| `parse_zero_patch` | `"v1.0.0"` | `Some((1, 0, 0))` |
| `parse_no_v_prefix` | `"2.3.4"` | `Some((2, 3, 4))` |
| `parse_empty` | `""` | `None` |
| `parse_garbage` | `"not-a-version"` | `None` |
| `parse_too_few_parts` | `"v1.2"` | `None` |

**Pass criteria:** All seven tests pass with exit code 0.

**Execution:**

```sh
cargo test -p rustycan -- updater::tests
```