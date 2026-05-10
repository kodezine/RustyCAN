---
active: true
derived: false
iec62304-clause: 8.1.2
level: 9.1
links: []
normative: true
ref: ''
reviewed: 3GmYPGhEJYc2C1ogoX1K-JwMjHEctst8RiZYYkzvD_U=
type: functional
---

# KCAN Dongle Firmware Update

RustyCAN **shall** perform firmware updates of the KCAN Dongle via USB DFU using the KCAN binary protocol, and **shall** verify the cryptographic signature (Ed25519) of the firmware image before programming it to the device, rejecting images with an invalid signature.