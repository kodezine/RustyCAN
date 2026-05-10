---
active: true
derived: false
level: 2.12
links:
- SOUP018: to3rDfjUXemfHdgyIXVvh3X-tmjsZfPwS2gS-uEVLaU=
method: manual
normative: true
ref: ''
reviewed: PfRpfrzYREEeQkX7_nf8WuYZiCw2lwvzDqWBL1HM0no=
test-command: ''
---

# Hardware Timestamp Resolution Verification

**Objective:** Verify that KCAN Dongle timestamps in the JSONL log have 100 ns resolution.

**Preconditions:** RustyCAN connected to a KCAN Dongle. CAN bus with periodic frame traffic.

**Procedure:**

1. Capture at least 100 frames with the KCAN Dongle.
2. Stop the session and open the JSONL log.
3. Inspect the `timestamp` values of consecutive frames.
4. Compute the difference between consecutive timestamps for two frames known to arrive close together.

**Pass criteria:** Timestamp values are expressed with sub-microsecond granularity (100 ns LSB). Differences between close frames resolve to values less than 1 µs where expected from the known frame interval.