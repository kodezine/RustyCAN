---
active: true
derived: false
level: 2.7
links:
- SOUP013: nONJfIM6qkcRYtsuMO8VGBojciWMkGCpMVrGdoQvPWA=
method: manual
normative: true
ref: ''
reviewed: 57BemJcqr8rrxc7RFqnvGDMUU8amwer3nP8Q5iWXQPc=
test-command: ''
---

# Native Desktop GUI Launch

**Objective:** Verify that the RustyCAN desktop GUI starts and renders the Connect and Monitor screens.

**Preconditions:** RustyCAN installed or built from source. No adapter required.

**Procedure:**

1. Launch `rustycan` (no flags).
2. Observe the Connect screen.
3. Connect to any available adapter.
4. Observe the Monitor screen.

**Pass criteria:** The application window opens without error. The Connect screen renders adapter type and port selection controls. After connection the Monitor screen renders NMT node table, PDO panel, and SDO panel.