---
active: true
derived: false
doc:
  copyright: RustyCAN
  title: RustyCAN — SOUP Test Cases
level: 3.7
links:
- SOUP030: ajBHW7StjdwZfFFIAVtnEdJTqvujaJ3cNeJgBTVfu4w=
method: manual
normative: true
ref: ''
reviewed: HUV2jkNSWaL1_SZV7W2FcQUadLA6wOZPb8h0VHM3kM0=
test-command: |
  cargo clippy --manifest-path firmware/Cargo.toml -p dongle-h743 --target thumbv7em-none-eabihf -- -D warnings
---

# Dongle DAQ-Readiness Build Check

**Objective:** Verify the dongle firmware compiles with the >=256-entry CAN-RX buffer and RX drop counter.

**Pass criteria:** Clippy passes with no warnings for the thumbv7em target.