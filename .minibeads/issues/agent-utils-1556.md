---
title: 'discuss-drag-grip-mark: a visible grip on the draggable Discuss title bar'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-15T12:00:00.000000000+00:00
updated_at: 2026-11-15T12:00:00.000000000+00:00
---

# Description

Owner UI request 078 (fast-track, owner rule 286): #1552
discuss-docked-height-drag (landed 429cad63, live 2026-11-14 7:45 PM ET)
made the "Discuss agent" title bar of the docked half-height Discuss pane
draggable to resize the pane, but nothing on screen shows that it can be
dragged. Add a small grip mark (the usual short horizontal pill or
three-line handle) on that title bar, so the drag is discoverable. The
owner is fine with the grip mark.

# Design

- **Mark.** A three-line grip (`.discuss-grip`, `aria-hidden`, no id) sits
  in the `#discuss-head` flex row between the title and the actions in
  `web/voice.html`, drawn in `web/styles/voice/22-discuss.css` as three
  2px bars in the bar's own foreground family (`#9aa3b2` at rest, the
  title's `#e7e9ee` on hover, the `#7ab3e3` accent while the bar is
  active), so it reads on the bar in light and dark alike.
- **Shown only where the bar drags.** The grip is `display:none` by
  default and shown only under the same conditions as #1552's
  `ns-resize` cursor: the phone regime (`not ((min-width: 900px) and
  (pointer: fine))`) with the pane at half height
  (`#discuss-panel[data-sheet="half"]`). It never shows on the maximized
  full-height sheet, the desk overlay, or the side-docked column, where
  dragging the bar does nothing.
- **Part of the drag target.** The grip has no handler of its own: a
  press on it bubbles to the bar's #1552 pointer handler (it is not a
  button, so the buttons-never-drag rule is untouched), and the whole
  bar keeps its existing cursor and `touch-action`. As a flex child
  (`flex: 0 1 34px; min-width: 18px; margin-inline: auto`) it centres in
  the free space, can shrink on narrow screens, and can never overlap
  the title or the maximize/close buttons.
- **Nothing else moves.** No JavaScript change: #1552's drag, 6px tap
  threshold, button exclusion, keyboard separator on
  `role=separator`, the #1526 scroll-into-view grid, and the #1527
  bottom bar are all preserved by construction. No change to
  `vibe-talk/web/voice.js` or `web/src/app/main.js` (#624).

# Acceptance Criteria

- Phone (~390px), docked half-height: the grip is visible on the title
  bar, clear of the title text and the buttons; a drag starting on the
  grip resizes the pane like a drag starting anywhere else on the bar.
- At 320px the grip still shows and still overlaps neither the title
  nor the buttons.
- Full-height (maximized) sheet and desktop overlay: no grip.
- Desktop hover/active feedback stays consistent with the existing
  `ns-resize` treatment of the bar.
- The browser walk keeps every #1552/#1526 guarantee (dock geometry,
  drag keeping, keyboard resize, citation landing, full/close) with
  condition waits only (owner rule 346).

# Notes

Owner UI request 078; prior work reused: #1552 (the drag this makes
discoverable), #1531 discuss-top-bar-slim-fixed (the bar the grip sits
in), #1526 discuss-docked-pane-mobile (the dock), #1527 (bottom bar,
untouched).
