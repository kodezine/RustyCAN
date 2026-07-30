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

# Summit Adapter Support

RustyCAN **shall** support connection to a Summit adapter for CAN bus access.

On macOS and Windows the vendor-provided Summit driver is used. On Linux, the SocketCAN kernel interface (`pcan` module) is used instead.