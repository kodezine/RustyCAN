---
active: true
derived: false
level: 2.2
links:
- SOUP003: srULBWyD6Am61fmZzwHaKSHX8wRo7LpLu75pI0wKgrA=
method: manual
normative: true
ref: ''
reviewed: Cu8__2ZYEgqYlsVPHRMFalL6Hp56K2m3sC9PuXEbV0k=
test-command: ''
---

# KCAN Dongle Connection

**Objective:** Verify that RustyCAN detects and connects to a KCAN Dongle and exposes both CAN channels.

**Preconditions:** KCAN Dongle (STM32H753ZI) connected via USB. No other software accessing the device.

**Procedure:**

1. Launch RustyCAN.
2. On the Connect screen, select adapter type **KCAN Dongle**.
3. Select baud rate and click **Connect**.

**Pass criteria:** The Monitor screen opens showing both FDCAN1 (channel 0) and FDCAN2 (channel 1). The status bar shows the dongle firmware version. No error dialog is displayed.