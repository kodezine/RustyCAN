---
active: true
derived: false
iec62304-clause: 8.1.2
level: 2.3
links: []
normative: true
ref: ''
reviewed: ck5dCzBhnxj933kqksyNQKwSd5fAyW7Ip-hBXPZGVKo=
type: functional
---

# Adapter Disconnection Detection

RustyCAN **shall** detect when a connected adapter is removed or becomes unresponsive during an active session and **shall** surface an `AdapterDisconnected` event to the user interface.