---
active: true
derived: false
doc:
  copyright: RustyCAN
  title: RustyCAN — Software of Unknown Provenance (SOUP)
iec62304-clause: 8.1.2
level: 12.6
links: []
normative: true
ref: ''
reviewed: ajBHW7StjdwZfFFIAVtnEdJTqvujaJ3cNeJgBTVfu4w=
type: functional
---

# Dongle CAN-RX Buffering for High-Rate DAQ

The RustyCAN dongle firmware **shall** buffer received CAN frames in a queue of at least 256 entries between the CAN-RX path and the USB transmit path, and **shall** maintain a cumulative counter of frames dropped due to queue saturation, so that sustained high-rate XCP DAQ capture does not silently lose frames.