---
active: true
derived: false
iec62304-clause: 8.1.2
level: 2.2
links: []
normative: true
ref: ''
reviewed: srULBWyD6Am61fmZzwHaKSHX8wRo7LpLu75pI0wKgrA=
type: functional
---

# KCAN Dongle Adapter Support

RustyCAN **shall** support connection to a KCAN Dongle (STM32H753ZI-based custom hardware) via USB bulk transfer using the `nusb` pure-Rust USB library.

The dongle exposes two CAN channels (FDCAN1 on channel 0, FDCAN2 on channel 1) using the KCAN binary protocol over USB bulk endpoints (OUT: 0x01, IN: 0x81).