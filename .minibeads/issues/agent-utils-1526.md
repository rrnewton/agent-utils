---
title: 'discuss-docked-pane-mobile: the half-height Discuss sheet on a phone is a docked pane, not an overlay'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-14T15:46:00.000000000+00:00
updated_at: 2026-11-14T15:46:00.000000000+00:00
---

# Description

Owner UI request 055. On a phone, with Discuss open at half height, pressing
a citation link to a message far up lands the "Linked message" row at the top
edge of the Discuss panel, mostly hidden behind it, and the upper list cannot
be scrolled further to reveal it. The panel is a floating overlay
(`position: absolute; height: 50dvh`) over `#scroll-area`, which keeps its
full height underneath: the scrollport's lower half is behind the sheet, so
`scrollIntoView` centring (and the list's true bottom) live in area the
reader cannot see.

# Design

CSS only, in `vibe-talk/web/styles/voice/22-discuss.css`, scoped to the phone
regime (`not ((min-width: 900px) and (pointer: fine))`): while the panel is
open at half height (`#discuss-panel:not([hidden])[data-sheet="half"]`),
`#screen-main` gains a fourth grid row (`minmax(0, 50dvh)`) and the panel
stands in it (`position: relative`), so `#scroll-area` (row 3) ends at the
panel's top edge — a layout split, not an overlay. The scrollport is then
exactly the visible upper area, so the existing scroll owner's
`scrollIntoView` jump and the list's bottom are correct with no jump-code
change. `#scroll-tools`, anchored to the bottom of `#screen-main`, rides
above the docked sheet. The full-height sheet still covers the main area,
a closed panel reserves no row, the resize handle and height presets are
untouched, and desktop behaviour is unchanged (a desk already docks
sideways, `#1354`).

# Acceptance Criteria

- Browser (`vibe-talk/tests/discuss_dock_browser.py`, 390x844): half-height
  Discuss open — the panel is in the layout, `#scroll-area`'s bottom is the
  panel's top, the list scrolls to its last row inside the visible upper
  area, a citation tap to the oldest seeded message lands its row fully
  inside the visible list area with the panel still open, full-height and
  closed states behave as before.
- `vibe-talk/tests/discuss_browser.py`: the desk walk still asserts the
  overlay leaves `#scroll-area` unmoved; the phone walk asserts the docked
  geometry instead.
- Desktop (1280x800) overlay unchanged: scroller unmoved on open.

# Notes

Coordinated with #1527 discuss (mobile Summarize bottom bar): that change
makes the panel a flex column on mobile, which composes with the docked
grid row; neither touches the other's selectors. #1524 and #1525 touch the
panel's bound row and heading line only.
