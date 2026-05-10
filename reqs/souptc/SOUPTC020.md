---
active: true
derived: false
level: 2.11
links:
- SOUP017: 3GmYPGhEJYc2C1ogoX1K-JwMjHEctst8RiZYYkzvD_U=
method: manual
normative: true
ref: ''
reviewed: aDE8M6So_Xe0ajkUBZ1yYyVJaPPH0zl0LXKdhJI2MR0=
test-command: ''
---

# KCAN Dongle Firmware Update via DFU

**Objective:** Verify that a signed firmware image can be successfully flashed to a KCAN Dongle via USB DFU.

**Preconditions:** KCAN Dongle connected via USB. A valid Ed25519-signed firmware image (`.bin`) available.

**Procedure:**

1. Launch `rustycan --dfu-firmware <signed.bin>` or initiate DFU from the GUI/TUI DFU flow.
2. Confirm the signature verification prompt.
3. Observe the update progress.
4. After completion, power-cycle the dongle and reconnect.

**Pass criteria:** The firmware update completes without error. The dongle boots the new firmware. An image with an invalid signature is rejected with an explicit error message before any programming begins.