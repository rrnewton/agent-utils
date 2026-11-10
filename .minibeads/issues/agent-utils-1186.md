---
title: "two-column-splitter: drag the middle separator to change the two columns' widths"
status: open
priority: 1
issue_type: task
created_at: 2026-11-10T03:19:34.000000000+00:00
updated_at: 2026-11-10T03:19:34.000000000+00:00
---

# Description

Owner reply 285 (2026-11-09 8:59 PM ET): "Another small user
interface thing: in the two-column mode, let's go ahead and make
it possible to grab and drag the middle separator to change the
width of the two columns." Owner reply 286: owner UI changes are
fast-tracked, so move quickly.

Today the desk two-column layout (Main left, thread or cards
right, `data-columns="two"` on #screen-main) is a fixed 50/50
split decided in CSS. The reader cannot give one column more
room than the other.

# Design

A draggable splitter on the separator between the two columns,
following the reading-width grip's shape (pointer events,
`role=separator`, kept per device):

- One separator element between #scroll-area and #side-column,
  shown only while two columns are on screen. Pointer events
  (`pointerdown`/`pointermove`/`pointerup` with pointer capture)
  so mouse, touch and pen share one path; `cursor: col-resize`
  on hover and a visible grab affordance at rest.
- The split is a percentage of the main screen's width, applied
  as a CSS custom property the two-column grid reads. Each
  column keeps a minimum width; the drag clamps there. Bounds
  and the default (50) live with the other numeric setting
  bounds, and the choice is kept per device with the other
  client settings so it survives a reload.
- Double-click on the separator resets to the default split.
  When the separator has focus, arrow keys move it
  (`role=separator`, `aria-valuenow` carrying the percentage).
- No scroll while dragging: the drag writes no scroll offset
  except the anchor keep #1144 sanctions — each column's place
  is read before the width changes and put back after, so the
  anchor row stays at the same viewport height while rows
  reflow. No other scroll writer runs during the drag.

Files: the setting's bounds and device entry in
`vibe-talk/web/src/core/settings/`, the drag owner and its
wiring in `vibe-talk/web/src/view/channel/` beside the
two-columns layout, the element and its ids in
`vibe-talk/web/voice.html` and
`vibe-talk/web/src/view/dom/ids.ts`, the grid and affordance in
`vibe-talk/web/styles/voice/20-desktop-regime.css`, page glue in
`vibe-talk/web/voice.js`, and tests in
`vibe-talk/tests/js/page/columns.mjs` plus a new browser walk
`vibe-talk/tests/two_column_splitter_browser.py`. The jump-chip
and read-toggle lanes touch neighbouring channel-view files;
whoever lands second rebases.

# Acceptance Criteria

- Dragging the separator with mouse, touch or pen changes the
  columns' widths live, with a visible grab affordance and a
  `col-resize` cursor on hover.
- Neither column goes below its minimum width however far the
  pointer travels; double-click restores the default split.
- The split persists per device across a reload, alongside the
  other client settings.
- Dragging scrolls neither column: each column's anchor row is
  at the same viewport position before and after the drag
  (within a pixel) while rows reflow, and no scroll writer
  other than the anchor keep runs.
- With the separator focused, arrow keys move the split and
  `aria-valuenow` reports it; page tests pin the drag math,
  the clamp, the reset and the keys, and a browser walk drags
  the splitter and asserts both columns' anchor rows are
  unchanged.

# Notes

Context: #1144 part 2a (on main, 716e218f) edited the
two-column layout; 2b (scroll trace) is in a combined gate now.
Parallel owner-UI lanes nearby: #1167 jump-to-unread-chip
(mu-layout), #1168 read-toggle-two-way (mu-readtoggle).
