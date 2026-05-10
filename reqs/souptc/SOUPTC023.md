---
active: true
derived: false
level: 2.14
links:
- SOUP021: bPjHpW2D2oY1z5KVjV_f7gq8SwXIV1PZNW2MJTtBtpQ=
method: automated
normative: true
ref: ''
reviewed: 1wWAVNYIhtcxfGDfEqZseSutYqSkUVvB9pDAhjXHf6Y=
test-command: (see CI workflows .github/workflows/ci.yml and release.yml)
---

# Cross-Platform CI Build and Test

**Objective:** Verify that RustyCAN builds and all automated tests pass on all supported platforms.

**Test suite:** GitHub Actions CI matrix — `ci.yml` (build + `cargo test`) on:

- macOS (Apple Silicon)
- macOS (Intel x86-64)
- Windows 10 x86-64
- Linux x86-64 (Ubuntu)

**Pass criteria:** All CI matrix jobs complete with exit code 0. The `release.yml` workflow produces installable artifacts for all three platforms (DMG, NSIS installer, AppImage/deb).