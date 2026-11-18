---
title: "vibe-talk-save-chip-compact-layout: the chip reserve and the keyboard Send in the compact layout"
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-18T17:27:47.000000000-05:00
updated_at: 2026-11-18T17:27:47.000000000-05:00
---

# Description

From the liaison's review of the #1867 matrix on a phone with the keyboard open (compact layout, dark and light): the Save draft chip's 7rem end reserve does not apply there — the keyboard-open rule for the focused box sets `padding-right: 3.25rem` (the new-line key's room) over it, so a long line runs under the chip's left edge. Beside the last line, the floating keyboard Send (`#keyboard-send`) also stands over the box's right edge where the words run.

# Design

- In the compact keyboard layout, the channel box keeps the chip's room too: the focused box's end padding is the chip's 7rem, which also covers the new-line key's 3.25rem. The side and reply boxes keep theirs (no chip stands over them).
- With that reserve, the last line's words wrap before the region the floating Send stands over, so Send is over the box's reserved strip, never over text.
- The save-draft chip browser check gains the keyboard-open phone state (viewport shrunk 336px, visualViewport verified): the chip and the keyboard Send both clear of the text area.

# Acceptance Criteria

- The extended browser check passes: keyboard-open phone (and the standing 412x915 / 1280x800 states) with chip and keyboard Send clear of typed text.
- The keyboard and composer walks still pass.

# Notes

Fast follow to #1867 vibe-talk-save-draft-chip-covers-text from the liaison's matrix review. Owner-priority, path-selected gate.
