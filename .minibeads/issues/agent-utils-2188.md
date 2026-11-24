---
title: 'vibe-talk-wtf-popover-below-chip: the WTF popover opens below the chip it spawned from, never overlapping it'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-24T05:30:00-05:00
updated_at: 2026-11-24T05:30:00-05:00
---

# Description

Owner 339 (2026-11-24): the WTF popover opens on top of the WTF? chip it spawned from. The popover must open below the chip, never overlapping it, clamped inside the viewport; if there is no room below, above the selection, still never over the chip.

# Acceptance Criteria

- Popover top edge is below the spawning chip's bottom edge (plus a small gap) whenever it opens from the chip.
- Popover is clamped inside the viewport horizontally and vertically.
- Fallback (no room below): popover above the selection, still never overlapping the chip.
- Screenshot scenes show the popover below the chip on phone 412 and desktop 1280, light and dark.
- Page tests for the exact placement.

# Notes

Owner UI request (fast-track, owner rule 286). Prior work: #2156 vibe-talk-wtf-glossary-lookup.
