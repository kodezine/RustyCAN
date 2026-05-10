---
active: true
derived: false
iec62304-clause: 8.1.2
level: 10.1
links: []
normative: true
ref: ''
reviewed: to3rDfjUXemfHdgyIXVvh3X-tmjsZfPwS2gS-uEVLaU=
type: performance
---

# Hardware Timestamp Resolution

When used with a KCAN Dongle, RustyCAN **shall** record CAN frame timestamps with a resolution of 100 nanoseconds, latched at frame Start-of-Frame (SOF) by the FDCAN RXTS hardware counter on the STM32H753ZI.