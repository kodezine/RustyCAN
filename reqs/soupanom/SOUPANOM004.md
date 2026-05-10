---
active: true
derived: false
iec62304-clause: 8.1.3
level: 1.4
links:
- SOUP002: ZAR-Jjld0uq4ky-9kJz75Rk8LCIWgxS8BWPrmMDxF94=
normative: true
ref: ''
reviewed: ds35epD2ph81jLmftZeJShuj88HfxHNicmooQtnW5iw=
type: anomaly
---

# PEAK Adapter Not Supported on Linux via Vendor Driver

**Affected platforms:** Linux

**Description:** The PEAK vendor-provided `pcan` userspace driver used on macOS and Windows is not available on Linux. Linux support for PEAK PCAN-USB adapters is provided via the SocketCAN kernel interface (`pcan` kernel module), which requires the `pcan` module to be loaded.

**Workaround:** On Linux, configure the PEAK adapter as a SocketCAN network interface (e.g., `ip link set can0 up type can bitrate 250000`) before launching RustyCAN.

**Impact:** Linux users must perform additional system configuration before connecting a PEAK adapter. KCAN Dongle support is unaffected on all platforms.