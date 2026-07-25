---
active: true
derived: false
doc:
  copyright: RustyCAN
  title: RustyCAN — Software of Unknown Provenance (SOUP)
iec62304-clause: 8.1.2
level: 12.2
links: []
normative: true
ref: ''
reviewed: E-LKY-T3qu98zKerEHkr6w_p90HrWFrrt1TYfhhVKfs=
type: interface
---

# XCP-on-CAN Transport and CRO/DTO Routing

RustyCAN **shall** decode ASAM MCD-1 XCP-on-CAN traffic on operator-configured CRO (master to slave) and DTO (slave to master) CAN identifiers, classifying each DTO packet as command response, error, event, service request, or DAQ data, and **shall** recognise XCP frames only on the configured identifiers so that the overlapping CANopen SDO identifier range is not misclassified.