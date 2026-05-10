---
active: true
derived: false
level: 2.9
links:
- SOUP015: iIOpxbySDPAQZvXrRqRdbsaLr1Ux4XFb3CUqY70CxeM=
method: manual
normative: true
ref: ''
reviewed: e9HpM_91r-uTvamJkq6HXwngmIKNhzXowft0_isbUgc=
test-command: ''
---

# Live HTTP/SSE Dashboard

**Objective:** Verify that the live browser dashboard is accessible and streams real-time events.

**Preconditions:** RustyCAN connected to a CAN bus with active traffic.

**Procedure:**

1. Start a session in GUI or TUI mode.
2. Open `http://127.0.0.1:7878/` in a web browser.
3. Observe the page for at least 10 seconds.

**Pass criteria:** The page loads without error. The NMT node grid renders. New CAN events appear in the event log in real time via SSE without requiring a page refresh.