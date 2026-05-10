---
active: true
derived: false
iec62304-clause: 8.1.3
level: 1.5
links:
- SOUP017: 3GmYPGhEJYc2C1ogoX1K-JwMjHEctst8RiZYYkzvD_U=
normative: true
ref: ''
reviewed: 70t-dzgLO6F6x4gPSAleG1ZoV1rODcFdlvTjhw893gQ=
type: anomaly
---

# bbd Firmware Tool Excluded from Packaged Releases

**Affected platforms:** All

**Description:** The `bbd` (BinaryBlockDownload) command-line tool for CANopen firmware updates via PEAK or KCAN adapters is compiled as part of the workspace but is excluded from signed packaged releases (DMG, NSIS installer, AppImage/deb). It is available only in developer builds (`cargo build -p rustycan --bin bbd`).

**Workaround:** Build from source to obtain `bbd`. Pre-built packaged installers do not include this tool.

**Impact:** End-users installing RustyCAN from a release package cannot perform CANopen-based firmware updates using `bbd` without building from source. The KCAN Dongle USB DFU update path (via the GUI or `--dfu-firmware` flag) is unaffected and is included in all releases.