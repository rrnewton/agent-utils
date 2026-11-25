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
created_at: 2026-11-25T10:12:00-05:00
updated_at: 2026-11-25T11:50:00-05:00
---

# Description

Owner UI request 373 (2026-11-25 10:05 AM ET, owner-UI fast-track), part B. With a document open, the Documents reader's controls pop up as a two-row panel: on a desk the centred Reader box (#1544) wraps onto a second row for the two wide jump buttons that #2006 added ("Jump to last mention", "Jump to discussion"), and on a phone the jumps take a third row of their own under the control tile. The owner: "this is all around badly designed... these could be shorter length buttons and could easily fit docked on the one tool bar at the bottom in this desktop layout."

Part A of the same request (opening a document link returned non-JSON, HTTP 502) is #2241 vibe-talk-document-load-502-readable.

# Design

Approved by the coordinator and the liaison before code (2026-11-25).

- The two jumps keep their ids, aria-labels ("Jump to last mention", "Jump to discussion") and tooltips, and each becomes a compact quiet button (the bar's panel-and-edge dress, so the accent stays on Pace and Read): an icon (a speech bubble with a jump-back arrow; two overlapping bubbles) and one word, "Mention" and "Discussion". A `data-tip` draws the tooltip on keyboard focus as well, since `title` shows only under a mouse; the screen reader's name stays the aria-label.
- Desktop (900px and up, fine pointer): the #1544 Reader box stays, as ONE row at the bar's 36px height inside the bottom toolbar: Reader, Mention, Discussion, audio, Pace, Stop (while a read plays), Read, in markup and Tab order. The box is placed across the dock and centred by auto margins at its own width; placed from `left: 50%` it could only be as wide as the window's right half, which is what wrapped it onto a second row standing over the page. It never wraps, and its padding is cut so its buttons stand on the bar buttons' line.
- Narrow desk (900-1199px): the jumps drop their words (icon only) and the box centres in the band between the gear and the corner's hamburger, Outline and switch, so it never runs under them, Stop shown or not.
- Phone (under 900px or a touch pointer): the jumps leave their row for the tile's second narrow column, stacked like Sound and the audio toggle in the first, each an icon over a one-word caption; the tile's rows become 44px (the first column's pair were 38px), Pace and Read keep their words, and the dock is 160px instead of 194px with a document open. Every rule is scoped to the jumps being shown, so with no document open the tile is exactly as before.
- Files: `vibe-talk/web/voice.html`, `web/styles/voice/19-control-pane.css`, `web/styles/voice/20-desktop-regime.css`, the no-discussion notice's words in `web/src/view/documents/index.ts`; the real-server check `tests/documents_control_bar_browser.py`; screenshot scenes 118 and 119 and an opt-in desk-1000 profile in the shared harness (`scripts/screenshots.py`), whose `scripts/run.sh` gives the throwaway server a `[documents]` paste section and a plain message cache only when one of those scenes runs.

# Acceptance Criteria

- Desktop (1280x800 and the narrowest desk width): with a document open, the reader controls are one row inside the one bottom toolbar; no second row, no panel taller than the bar; nothing overlaps the corner buttons (hamburger, Outline, view switch).
- Each jump button is compact (an icon plus at most one short word, icon only where room is short), has a tooltip and an aria-label, and still performs its action (Jump to last mention lands on the mentioning message; Jump to discussion on the discussion or the no-discussion notice).
- Phone (412x915): the jumps no longer take a row of their own; every reader control's touch target is at least 44 px; the tile still fits with Stop shown while a read plays.
- Tab reaches every control in visual order, Enter and Space activate them, and the existing keyboard shortcuts (Read, outline g, and the rest) still work.
- Page tests cover each action from its new place at phone and desktop widths; screenshots before (main) and after, desktop and phone, dark and light, with a document open in the reader, plus the standard matrix, each folder with its BUILD.txt.

# Notes

Owner request 373, part B (cl-docbar). Related: #2006 (added the jumps), #1544 (the desktop Reader box), #1545, #1542, #2033, #2058.
