---
active: true
derived: false
level: 2.8
links:
- SOUP014: ShvkrFOUCLnj7e6uCmbUThrVKbjzozIbNgFqVaRyYzs=
method: manual
normative: true
ref: ''
reviewed: aDRp5W24jF-zgFpdKmnlmaqmE0tWKU8ctrZ0BSic8A4=
test-command: rustycan --tui --config <config.json>
---

# TUI Mode Launch

**Objective:** Verify that the `--tui` flag starts the full-screen terminal user interface.

**Preconditions:** A valid JSON config file pointing to an available adapter. Terminal capable of rendering full-screen TUI (e.g., 80×24 minimum).

**Procedure:**

1. Run `rustycan --tui --config <config.json>`.
2. Observe the terminal.
3. Press `q` to quit.

**Pass criteria:** A full-screen terminal UI is rendered showing NMT, PDO, SDO, and event-log panels. The terminal is fully restored to normal mode after quitting. No graphical window is opened.