---
active: true
derived: false
level: 2.13
links:
- SOUP020: uf3gOJ7c-S7zm-6M6TxghXmM1xV-ibBD86CWXfupWqc=
method: manual
normative: true
ref: ''
reviewed: dA3FG2M_m85MsWuvCTxuGDRMBKk-A0zuQhnapUMyJ3k=
test-command: ''
---

# Multi-Node Logging Stability

**Objective:** Verify that RustyCAN sustains continuous logging without frame loss on a network with seven or more active CANopen nodes.

**Preconditions:** Seven (7) or more CANopen nodes active on the bus, each transmitting heartbeats and at least one TPDO. Adapter: PEAK or KCAN.

**Procedure:**

1. Start a session with all seven nodes active.
2. Allow the session to run for 60 seconds.
3. Stop the session and open the JSONL log.
4. Count heartbeat events per node and compare against expected heartbeat count (node heartbeat interval × 60 s).

**Pass criteria:** Heartbeat frame counts per node deviate by no more than 2% from the expected count. No corruption or truncated JSON lines are present in the log.