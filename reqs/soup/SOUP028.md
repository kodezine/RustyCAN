---
active: true
derived: false
doc:
  copyright: RustyCAN
  title: RustyCAN — Software of Unknown Provenance (SOUP)
iec62304-clause: 8.1.2
level: 12.4
links: []
normative: true
ref: ''
reviewed: m8ZsxhlNNohAlbVRHP2jpUW6sH_sNrmc6-r83M-dk_g=
type: functional
---

# XCP DAQ Measurement Capture

RustyCAN **shall** reconstruct XCP DAQ measurement layouts by observing the dynamic DAQ configuration command sequence (SET_DAQ_PTR, WRITE_DAQ, START_STOP_DAQ_LIST) and **shall** decode DTO DAQ frames into address-keyed measurement values using the resulting PID-to-ODT map.