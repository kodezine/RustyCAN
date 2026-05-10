---
active: true
derived: false
level: 2.1
links:
- SOUP002: ZAR-Jjld0uq4ky-9kJz75Rk8LCIWgxS8BWPrmMDxF94=
method: manual
normative: true
ref: ''
reviewed: FJTXHqXtuzDQ3zwr-jwNNcY1jx6YBt3fwruZSK9x5UQ=
test-command: ''
---

# PEAK PCAN-USB Adapter Connection

**Objective:** Verify that RustyCAN detects and connects to a PEAK PCAN-USB adapter.

**Preconditions:** PEAK PCAN-USB adapter connected to host. Driver installed (macOS/Windows) or `pcan` kernel module loaded (Linux). CAN bus with termination resistors.

**Procedure:**

1. Launch RustyCAN.
2. On the Connect screen, select adapter type **PEAK PCAN-USB**.
3. Select the correct port/channel and baud rate.
4. Click **Connect**.

**Pass criteria:** The Monitor screen opens. The status bar shows the adapter as connected. No error dialog is displayed.