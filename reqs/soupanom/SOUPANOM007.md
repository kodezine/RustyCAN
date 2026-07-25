---
active: true
derived: false
doc:
  copyright: RustyCAN
  title: RustyCAN — SOUP Anomaly List
iec62304-clause: 8.1.3
level: 1.7
links:
- SOUP027: 3dRCZxWzcj2bKOqmWlok0VgIIkF5kyK246X_Mv54zo0=
normative: true
ref: ''
reviewed: AkYB892hdTsrufMn6euwIbRNh8WR2TnBDWRhrF7xdKE=
type: anomaly
---

# XCP Seed and Key Requires Manual Unlock

**Description:** RustyCAN XCP master does not embed a vendor seed-and-key algorithm. For slaves that lock resources, the GET_SEED value is surfaced and the operator must supply the corresponding UNLOCK key bytes manually.

**Impact:** Locked resources cannot be accessed without the externally-computed key. No impact on unlocked slaves.