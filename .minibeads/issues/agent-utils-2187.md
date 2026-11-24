---
title: 'vibe-talk-wtf-chips-one-row-clear-of-handles: the selection chip bar stays one row, inside the viewport and clear of the selection handles'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-24T05:30:00-05:00
updated_at: 2026-11-24T05:30:00-05:00
---

# Description

Owner 339 (2026-11-24): selecting words at the left edge of a message wraps the chip bar (WTF?, Quoted reply, x) onto two rows, clips WTF? at the screen edge, and covers the selection's end handle. The bar must be ONE horizontal row, never wrapping, placed below the selection, clamped inside the viewport (with safe-area insets), never overlapping the selection handles (clearance below the selection's last line; above the selection, clear of the start handle, when there is no room below), and clear of the native selection toolbar.

# Acceptance Criteria

- Chip bar (WTF?, Quoted reply, x) renders as one horizontal row at every viewport width and selection position.
- Bar is clamped inside the viewport horizontally; no chip is clipped.
- Bar never overlaps the selection handles; below-selection placement by default, above when no room below.
- Screenshot scenes: selection at left edge, right edge, near viewport bottom, and multi-line, on phone 412 and desktop 1280, light and dark, keyboard open and closed.
- Page tests at phone and desktop sizes for the exact defect.

# Notes

Owner UI request (fast-track, owner rule 286). Prior work: #2156 vibe-talk-wtf-glossary-lookup.

## Resolution

[watermelon-full-preview] Landed in the chip stack on main as 5434da2b (full gate PASS 225/225 on a3396451). The chip bar is one clamped row below the selection, clear of the handles, with the solid strip and rigid container (#2220) completing the row. #2187 vibe-talk-wtf-chips-one-row-clear-of-handles.
