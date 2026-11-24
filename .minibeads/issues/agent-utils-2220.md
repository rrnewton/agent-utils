---
title: 'vibe-talk-selection-chip-rigid-row: WTF?, Quoted reply and X are one rigid chip-group container'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-24T14:20:00-05:00
updated_at: 2026-11-24T14:20:00-05:00
---

# Description

Owner 368 (2026-11-24, verbatim): "Ugh I'm still watching that WTF chip bounce all over. I hope a change is in the works to make it a rigid grid with the (Quoted Reply) and (X)."

The selection chips (WTF?, Quoted reply, dismiss X) are three separately positioned buttons. Even with one shared row geometry (#2187 vibe-talk-wtf-chips-one-row-clear-of-handles), WTF? is written by a second placement pass and can trail the row, and its late appearance reflows the row. The chips must be ONE container element, positioned once, with a flex row inside.

# Design

- One container element wraps WTF?, Quoted reply and X; a single left/top (or transform) on the container positions the whole row; no chip gets its own left/top writes.
- The container shows and hides as one unit. WTF? appearing late (its lookup resolving after the other chips) must not move the container or its siblings: its slot is reserved.
- The glossary view stops positioning WTF?; it only toggles its content/visibility.

# Acceptance Criteria

- One container, one position write per placement; no separate left/top for any chip.
- Browser walk: the three chips keep constant offsets from each other across scroll, re-render, fold/unfold and selection change; the container rect does not change when WTF? reveals late.
- Shots: phone dark and light, desktop, before and after a re-render and a scroll, chip row visible in both frames of each pair.

# Notes

Owner UI request (fast-track, owner rule 286). Follow-up to #2187 vibe-talk-wtf-chips-one-row-clear-of-handles and #2195 vibe-talk-wtf-chip-orphan-lifecycle.

## Resolution

[watermelon-full-preview] Landed in the chip stack on main as 5434da2b (full gate PASS 225/225 on a3396451). One #selection-chip-row container positioned once, WTF? slot reserved, strip always under the selection (owner 339/368). #2220 vibe-talk-selection-chip-rigid-row.
