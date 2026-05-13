---
active: true
derived: false
iec62304-clause: 8.1.2
level: 12.1
links: []
normative: true
ref: ''
reviewed: l_BpydkvPLEDKrRVfYVRmmCcZTsJ58j8FFbRXs3_62o=
type: functional
---

# App Update Notification and Self-Update

RustyCAN **shall** check for a newer released version at startup by querying the GitHub Releases API (`GET /repos/kodezine/RustyCAN/releases/latest`).

If a newer version is detected:

- The user **shall** be notified in all modes (GUI, TUI, log-stream).
- The user **shall** be offered the option to **update now** or **defer**.
- **Defer** dismisses the notification for the current session only; the notification reappears on the next launch.

Update behaviour by platform:

| Platform | Behaviour |
|---|---|
| macOS Apple Silicon (aarch64) | Downloads the release DMG, mounts it, replaces the running `.app` bundle in-place, and relaunches the new version automatically. |
| Windows (x86-64) | Presents a labelled button and hyperlink to the GitHub release page for the available version. |
| Linux (x86-64) | Presents a labelled button and hyperlink to the GitHub release page for the available version. |

The version check **shall** be non-blocking; any failure to reach the GitHub API (network unavailable, rate-limit, parse error) **shall** be silently ignored and **shall not** prevent normal operation.

The version comparison **shall** parse Git tags of the form `vMAJOR.MINOR.PATCH` (with an optional `-N-gHASH` describe suffix) and compare them against the build-time version embedded at compile time.