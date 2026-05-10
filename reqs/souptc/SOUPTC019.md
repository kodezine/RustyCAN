---
active: true
derived: false
level: '2.10'
links:
- SOUP016: OwItojX-qyzb_CSGGsSD4kGiJMtX5-JhQUAgT8BDrP8=
method: manual
normative: true
ref: ''
reviewed: ezKSD1kwe0TmGIloFcIEayyobsdDn7D9i4ONkfMaWYY=
test-command: rustycan --config config.kcan.json
---

# JSON Configuration File Loading

**Objective:** Verify that adapter type and baud rate specified in a JSON config file are correctly applied on startup.

**Preconditions:** A valid `config.kcan.json` file specifying KCAN adapter and a specific baud rate.

**Procedure:**

1. Run `rustycan --config config.kcan.json`.
2. Observe the Connect screen (GUI) or the session startup (TUI/log mode).

**Pass criteria:** The adapter type shown matches the config file. The baud rate shown matches the config file. No prompts for adapter selection are shown when `auto_connect` is set in the config.