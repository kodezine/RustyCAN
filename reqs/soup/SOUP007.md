---
active: true
derived: false
iec62304-clause: 8.1.2
level: 4.1
links: []
normative: true
ref: ''
reviewed: ji-5u2RzHuCwzihEuq4KEiVzAQXNlzSUS4BTeELb63Y=
type: functional
---

# CANopen NMT Node State Monitoring

RustyCAN **shall** monitor the NMT (Network Management) state of CANopen nodes on the bus by processing heartbeat messages (COB-ID 0x700 + node-ID) as defined in CiA 301, and **shall** track the last-seen state and inter-event period for each node.