---
active: true
derived: false
level: 2.3
links:
- SOUP004: ck5dCzBhnxj933kqksyNQKwSd5fAyW7Ip-hBXPZGVKo=
method: manual
normative: true
ref: ''
reviewed: q7u0QUIrwiPtGmkIRnGumSBVFgItRwL01RGRDo8Qmso=
test-command: ''
---

# Adapter Disconnection Detection

**Objective:** Verify that RustyCAN detects and surfaces an adapter disconnection event during an active session.

**Preconditions:** RustyCAN connected to any supported adapter with an active session.

**Procedure:**

1. Establish a connection and confirm the Monitor screen is active.
2. Physically unplug the adapter USB cable.
3. Observe the UI within 5 seconds.

**Pass criteria:** The UI displays an `AdapterDisconnected` notification (error banner or status change). The application does not crash. The user can dismiss the error and return to the Connect screen.