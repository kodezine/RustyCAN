---
active: true
derived: false
level: 2.5
links:
- SOUP010: SXfqm84Qq2a6KjUF_TwlUpwnw6x6ud2Ga1WRQH-eQA0=
method: manual
normative: true
ref: ''
reviewed: actgoJWMa4cA3Andqv-YSRexV-SqFzZ-pkzAilesauM=
test-command: ''
---

# SDO Read and Write on Live Node

**Objective:** Verify that SDO upload (read) and download (write) operations return correct values on a live CANopen node.

**Preconditions:** RustyCAN connected. A CANopen node with a known object dictionary accessible via SDO (e.g., Device Type at 0x1000 sub 0).

**Procedure:**

1. Using the SDO panel, request an SDO read on index `0x1000`, sub `0`.
2. Observe the returned value.
3. Request an SDO write on a writable object (e.g., a vendor-specific parameter).
4. Perform an SDO read of the same object to confirm the written value.

**Pass criteria:** The read returns the expected value. The write completes without error. The subsequent read confirms the written value is stored.