---
active: true
derived: false
iec62304-clause: 8.1.3
level: 1.3
links:
- SOUP003: srULBWyD6Am61fmZzwHaKSHX8wRo7LpLu75pI0wKgrA=
normative: true
ref: ''
reviewed: q2Aidzvq5hYE2D5rV_5u4aLWdh7Gl8ks2DUE4QnCW5o=
type: anomaly
---

# OTG FS: Bulk OUT Writes >64 Bytes Silently Truncated

**Affected platforms:** All (firmware behaviour; visible on host regardless of OS)

**Description:** On the STM32H753ZI OTG FS peripheral, a single `write_packet` call with a payload larger than 64 bytes (the USB Full-Speed Maximum Packet Size) silently truncates the transfer to 64 bytes. This affects KCAN command frames that exceed one USB packet.

**Workaround:** The KCAN firmware transmits all bulk OUT data in 64-byte (MPS) chunks, appending a zero-length packet (ZLP) when the payload is an exact multiple of MPS. The host bulk IN read buffer is sized to a multiple of MPS (128 bytes for 80-byte KCAN frames).

**Impact:** None at runtime with the workaround applied. Removing the chunking from firmware would cause silent data loss.