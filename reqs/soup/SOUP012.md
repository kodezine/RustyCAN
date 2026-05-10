---
active: true
derived: false
iec62304-clause: 8.1.2
level: 6.1
links: []
normative: true
ref: ''
reviewed: di8u0yIr1oQmcLRho71B5SdmMa0MdBAYP_Avp9vcX_M=
type: functional
---

# JSONL Event Logging

RustyCAN **shall** record all CAN events — including raw frames, NMT state changes, PDO signal values, and SDO transactions — to a timestamped newline-delimited JSON (JSONL) file for the duration of each session.