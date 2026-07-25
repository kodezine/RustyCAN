---
active: true
derived: false
doc:
  copyright: RustyCAN
  title: RustyCAN — Software of Unknown Provenance (SOUP)
iec62304-clause: 8.1.2
level: 12.3
links: []
normative: true
ref: ''
reviewed: 3dRCZxWzcj2bKOqmWlok0VgIIkF5kyK246X_Mv54zo0=
type: functional
---

# XCP Master Command Set

RustyCAN **shall** provide an XCP master capable of issuing CONNECT, DISCONNECT, GET_SEED, UNLOCK, SET_MTA, UPLOAD, and DOWNLOAD commands over CAN, tracking a single in-flight transaction and honouring the slave byte order and MAX_CTO negotiated in the CONNECT response.