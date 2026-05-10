---
active: true
derived: false
iec62304-clause: 8.1.2
level: 2.1
links: []
normative: true
ref: ''
reviewed: ZAR-Jjld0uq4ky-9kJz75Rk8LCIWgxS8BWPrmMDxF94=
type: functional
---

# PEAK PCAN-USB Adapter Support

RustyCAN **shall** support connection to a PEAK PCAN-USB adapter for CAN bus access.

On macOS and Windows the vendor-provided PEAK driver is used. On Linux, the SocketCAN kernel interface (`pcan` module) is used instead.