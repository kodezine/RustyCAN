---
active: true
derived: false
iec62304-clause: 8.1.3
level: 1.8
links: []
normative: true
ref: ''
reviewed: q11juhCMcst_prgmy-jQy8Yyf0MWEyB87uviHraawp4=
type: anomaly
---

# Apex Userspace (nusb) Backend Unsuitable for Bulk SDO Download on Linux

**Affected platforms:** Linux (userspace `nusb` path). The throughput limit is inherent to the `nusb` backend on all platforms; only Linux offers a SocketCAN remedy.

**Description:** When the Apex USB-CAN adapter (SYS TEC USB-CANmodul, VID `0x0878`) is driven through RustyCAN's userspace `nusb` backend, large SDO transfers such as `bbd` firmware downloads are unreliable and impractically slow. The single-threaded reader loop issues one status-IN and one data-IN poll per iteration, so a strict request/response segmented download advances only about one CAN frame per poll cycle (~180 B/s — roughly 14 minutes for a 145 KB image). Block-mode transfers burst faster than the device's CAN-TX FIFO drains, producing SDO abort `0x05040003` (invalid sequence number). A CAN-bus capture confirms the bus, bootloader, and node are healthy — every segment is acknowledged with the correct toggle and no protocol errors — so the limitation is in the userspace USB backend, not the device or its firmware.

**Workaround:** On Linux, drive the SYS TEC device through its SocketCAN kernel driver instead of `nusb`. Load the vendor ATLAS `systec_can` module (build it against the running kernel with `make` if the prebuilt module reports `Invalid module format`); the device then appears as a `canX` interface with kernel-level flow control and downloads the full image reliably in about a minute. More generally, on Linux route all third-party USB-CAN modules through their SocketCAN kernel driver for firmware/bulk work. The `nusb` path remains suitable for live monitoring.

**Impact:** Linux users performing firmware updates or other bulk SDO transfers over an Apex adapter must use the SocketCAN kernel-driver path. Live monitoring over `nusb` is unaffected. Windows and macOS have no SocketCAN and continue to use the `nusb` path unchanged; for bulk work on those platforms prefer a kernel-driver-backed adapter where available.