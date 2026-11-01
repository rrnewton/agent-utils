---
title: 'vt-selection-quick-reply: a floating quick-reply box under the selection that never moves the page'
status: closed
priority: 1
issue_type: feature
created_at: 2026-11-01T16:07:43.114840631+00:00
updated_at: 2026-11-01T21:10:23.315250234+00:00
closed_at: 2026-11-01T21:10:23.315250114+00:00
---

# Description

[claude-opus-5-5] Owner GO 2026-11-01 10:40 ET. Tapping the vt-selection-chip opens a floating quick-reply box anchored under the selection, not the main composer. Send posts a quoted reply in that message's thread and closes the box. The scroll position and reading place stay exactly put: no jump to the composer, no re-scroll when the reply or its answer arrives. Several replies to different sentences of one long message in a row is the intended use. Esc or a tap outside closes it, keeping the draft for the same selection. Tests: scrollTop unchanged across open, send and arrival, at desk and phone sizes.
