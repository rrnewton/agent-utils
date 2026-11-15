---
title: 'discuss-grip-left: move the Discuss drag grip to the left end of the title bar'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-15T15:06:00.000000000+00:00
updated_at: 2026-11-15T15:06:00.000000000+00:00
---

# Description

Owner UI request 095 (fast-track, owner rule 286): the grip mark from
#1556 discuss-drag-grip-mark (landed e58a51b8, live) sits centred on the
docked half-height Discuss title bar and reads as floating randomly.
Move that little grip icon to the left of the bar.

# Design

- **Placement.** The `.discuss-grip` span moves before `.discuss-title`
  in `#discuss-head` in `web/voice.html`, first in the bar's flex row.
  In `web/styles/voice/22-discuss.css` it loses its centring
  (`order: 1; margin-inline: auto`) and keeps `flex: 0 1 34px;
  min-width: 18px` with `align-self: center`, so its left edge aligns
  with the bar's content padding (10px plus the safe-area inset), the
  row gap keeps it clear of the title text, and the bar's centred
  alignment centres it vertically. It can shrink but, as a flex item,
  can never overlap the title or the actions.
- **Unchanged behaviour.** No JavaScript change. The grip stays
  `display: none` by default and shows only where the bar drags (the
  phone regime, `#discuss-panel[data-sheet="half"]`): never on the
  maximized full-height sheet, the desk overlay, or the side-docked
  column. It keeps no handler of its own, so a press on it still
  bubbles to the bar's #1552 pointer handler and the grip remains part
  of the drag target. No change to `vibe-talk/web/voice.js` or
  `web/src/app/main.js` (#624).

# Acceptance Criteria

- Phone (~390px), docked half-height: the grip is visible at the left
  end of the title bar, its left edge on the bar's content padding,
  clear of and left of the title text, vertically centred in the bar;
  a drag starting on the grip resizes the pane like a drag starting
  anywhere else on the bar.
- At 320px the grip still shows at the left end and overlaps neither
  the title nor the buttons, in light and dark alike.
- Full-height (maximized) sheet and desktop overlay: no grip.
- The browser walk keeps every #1552/#1526 guarantee (dock geometry,
  drag keeping, keyboard resize, citation landing, full/close) with
  condition waits only (owner rule 346).

# Notes

Owner UI request 095; prior work reused: #1556 (the grip this moves),
#1552 (the drag it marks), #1531 discuss-top-bar-slim-fixed (the bar),
#1526 discuss-docked-pane-mobile (the dock). Sibling lanes from the
same owner message, landing independently: #1585 (expand the linked
message) and #1586 (full-screen Discuss drops to half on a message
link); this lane touches only the grip's markup, style, and dock walk.
