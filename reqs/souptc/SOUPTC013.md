---
active: true
derived: false
level: 2.4
links:
- SOUP008: sObYhdCaHSchNdbsuBNtW2THIf7vRY0ETuLSgUvx5r4=
method: manual
normative: true
ref: ''
reviewed: _LvMkD6ZawpxwuJohdX3UuCAKKYmR3f8VLxdcQm_sN8=
test-command: ''
---

# NMT Master Command Execution

**Objective:** Verify that NMT master commands issued via the GUI result in the correct node state transition on a live CANopen node.

**Preconditions:** RustyCAN connected. At least one CANopen node active on the bus, implementing standard NMT heartbeat (CiA 301).

**Procedure:**

1. Confirm the node appears in the Monitor screen in **Operational** state.
2. Issue **Stop Remote Node** to the node via the GUI.
3. Confirm node transitions to **Stopped**.
4. Issue **Start Remote Node** to the node.
5. Confirm node transitions back to **Operational**.
6. Issue **Reset Node** (broadcast, node-ID 0).
7. Confirm all nodes transition to **Bootup** then return to **Pre-Operational** or **Operational**.

**Pass criteria:** Each NMT state transition is reflected in the Monitor screen within 2 seconds of the command being issued.