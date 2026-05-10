---
active: true
derived: false
level: 2.6
links:
- SOUP012: di8u0yIr1oQmcLRho71B5SdmMa0MdBAYP_Avp9vcX_M=
method: manual
normative: true
ref: ''
reviewed: tIgz1fdXP9sJUGR6e1OBh0uo2YpGudc_Zb5EJBIyI2Q=
test-command: ''
---

# JSONL Event Log Integrity

**Objective:** Verify that the JSONL log file contains well-formed JSON entries with timestamps for all received CAN events.

**Preconditions:** RustyCAN connected to a CAN bus with at least one active node.

**Procedure:**

1. Start a session and allow at least 30 seconds of data capture.
2. Disconnect and locate the generated `.jsonl` log file.
3. Run `cat <logfile> | python3 -c "import sys,json; [json.loads(l) for l in sys.stdin]"` (or equivalent).
4. Inspect at least 10 entries for the presence of a `timestamp` field and a recognised event type field.

**Pass criteria:** All lines parse as valid JSON. Every entry contains a `timestamp` field. No truncated or malformed lines are present.