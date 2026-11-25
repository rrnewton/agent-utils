---
title: 'vibe-talk-documents-reader-toolbar: the Documents reader controls are compact buttons docked in the one bottom toolbar'
status: open
priority: 0
issue_type: feature
labels:
- vibe-talk
- documents
- reader
- ui
assignee: cl-docbar
created_at: 2026-11-25T10:30:00-05:00
updated_at: 2026-11-25T10:30:00-05:00
---

# Description

Owner UI request 373 (2026-11-25 10:05 AM ET, owner-UI fast-track), part B. With a document open, the Documents reader's controls pop up as a two-row panel: on a desk the centred Reader box (#1544) wraps onto a second row for the two wide jump buttons that #2006 added ("Jump to last mention", "Jump to discussion"), and on a phone the jumps take a third row of their own under the control tile. The owner: "this is all around badly designed... these could be shorter length buttons and could easily fit docked on the one tool bar at the bottom in this desktop layout."

Part A of the same request (opening a document link returned non-JSON, HTTP 502) is tracked separately.

# Design

Design note sent to the coordinator before any code; the settled design is recorded here when approved. Sketch: on a desk the jumps become compact icon buttons with one short word each, a tooltip and the existing aria-label, inside the Reader group, so the group is one row at the bar's height and stands in the one bottom toolbar; at narrow desk widths the words drop and the icons stay. On a phone the jumps leave their extra row for one compact place in the existing tile, every touch target at least 44 px. Every action, its keyboard reach and its screen-reader name stay as they are.

# Acceptance Criteria

- Desktop (1280x800 and the narrowest desk width): with a document open, the reader controls are one row inside the one bottom toolbar; no second row, no panel taller than the bar; nothing overlaps the corner buttons (hamburger, Outline, view switch).
- Each jump button is compact (an icon plus at most one short word, icon only where room is short), has a tooltip and an aria-label, and still performs its action (Jump to last mention lands on the mentioning message; Jump to discussion on the discussion or the no-discussion notice).
- Phone (412x915): the jumps no longer take a row of their own; every reader control's touch target is at least 44 px; the tile still fits with Stop shown while a read plays.
- Tab reaches every control in visual order, Enter and Space activate them, and the existing keyboard shortcuts (Read, outline g, and the rest) still work.
- Page tests cover each action from its new place at phone and desktop widths; screenshots before (main) and after, desktop and phone, dark and light, with a document open in the reader, plus the standard matrix, each folder with its BUILD.txt.

# Notes

Owner request 373, part B (cl-docbar). Related: #2006 (added the jumps), #1544 (the desktop Reader box), #1545, #1542, #2033, #2058.
