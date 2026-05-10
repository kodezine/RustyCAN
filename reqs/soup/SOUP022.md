---
active: true
derived: false
iec62304-clause: 8.1.2
level: 11.2
links: []
normative: true
ref: ''
reviewed: i-gGH4UqQmt1MPhvGxkCBPc25B12qLwmGUCeYY9u6Nc=
type: interface
---

# PEAK Adapter Hardware Interface

RustyCAN **shall** interface with PEAK PCAN-USB adapters via the vendor-provided PEAK driver on macOS and Windows, and via the SocketCAN kernel interface (using the `pcan` kernel module) on Linux.