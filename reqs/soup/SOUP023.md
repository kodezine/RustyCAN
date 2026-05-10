---
active: true
derived: false
iec62304-clause: 8.1.2
level: 11.3
links: []
normative: true
ref: ''
reviewed: Dobe1n9Xq81CkgTv5_OlWo-_c40cqZUzS61W_8oLlpo=
type: interface
---

# KCAN Adapter Hardware Interface

RustyCAN **shall** interface with the KCAN Dongle exclusively via the `nusb` pure-Rust USB library (no dependency on libusb or system USB frameworks), communicating over USB bulk endpoints OUT=0x01 and IN=0x81.