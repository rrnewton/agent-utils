---
title: 'switcher-frame-panel: frame the view switcher tab row so the selected cell cannot bleed into Talk'
status: open
priority: 1
issue_type: task
created_at: 2026-11-09T22:31:16.847801059+00:00
updated_at: 2026-11-09T22:31:16.847801059+00:00
---

# Description

Owner reply 278 (2026-11-09), from the voice view on Android: "That
little selector thing when I'm tapping the voice channel documents
three-way selector needs to have some kind of background frame
around it. Do you see in the screenshot how the blue of the voice
button that's selected just bleeds in to a talk button underneath
it."

The transient tab row from #1078 (Voice, Channel, Documents) floats
directly over the control pane's big Talk button. The selected Voice
cell is the same accent blue as Talk, the unselected cells are dark
with thin borders, and nothing frames the row, so the selected cell
and the button underneath read as one blue shape.

# Design

Give the tab row its own framed panel: an opaque background, a
border and shadow, a little padding inside the frame, and a small
gap between the row and what is under it. The selected cell keeps
its highlight but in a colour or with an outline that stays
distinct from Talk's blue, in dark and light schemes alike. Nothing
else about the row changes: same cells, same rotation, same settle
after the last press, same direct choice, same pointer behaviour
while a call can be hung up.

Files are expected to be the Documents stylesheet's row rules and
the page test that already covers the row; any shared shell or
script file will be cleared with the coordinator before editing.

# Acceptance Criteria

- Over the voice view's Talk button, the channel composer and the
  Documents view, at phone and desktop widths in dark and light,
  the row reads as one framed panel and the selected cell is
  unmistakable against what is under it.
- The row's behaviour is unchanged: stepping, wrapping, settling,
  dismissal and direct choice as #1078 delivered.
- A page test or browser screenshot check covers the framed row
  and the distinct selected cell.

# Notes

Owner reply 278, 2026-11-09 5:23 PM ET; follow-up to #1078
bottom-bar-switcher. Screenshot supplied with the request. To be
gated together with #1167 jump-to-unread-chip and #1168
read-toggle-two-way under one full gate after the coordinator's
all-clear.
