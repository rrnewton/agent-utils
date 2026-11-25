---
title: 'vibe-talk-selection-strip-atomic: the selection strip and all its chips appear in one frame, WTF never late or covered'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-25T06:30:00-05:00
updated_at: 2026-11-25T09:19:55-05:00
closed_at: 2026-11-25T14:19:55.000000000+00:00
---

# Description

Owner (request 371, 2026-11-25 6:19 AM ET): "Holy crap. Even with the new grouping bar under with wtf and quote reply (which I did not want but do not mind so we can leave it) they still appear async and I see it rendering without the wtf button. Or the wtf button appears later. I'm t should appear as tho it's one object. Basic. Maybe z order is wrong and sometimes the new background dock judges the icon itself."

Keep the selection strip (the owner accepts it). The strip (Quote reply, WTF?, dismiss X) must appear atomically, as one object, in the same frame. WTF? must never be missing, appear late, or be hidden behind the strip background. Follow-up to #2220 vibe-talk-selection-chip-rigid-row, #2216, #2195 and #2187.

# Design

- One owner for the strip group's visibility: the call that unhides the strip row also decides WTF?'s visibility, in the same synchronous step and frame. No other listener (the glossary's own selection-change path) decides WTF?'s visibility on its own or ahead of the strip's deferred draw.
- If a child depends on data that is not ready, its slot is reserved and rendered in the same frame (disabled if needed), never absent.
- Stacking: the strip background never paints over a child; children sit above the background inside one stacking context.

# Acceptance Criteria

- A page test asserts that in the same animation frame as the strip's first paint, Quote reply, WTF? and X are all present, visible and hit-testable (elementFromPoint at each centre returns that control, not the strip background), across first selection, selection change, re-selection and hot load, at phone 412x915 and desktop 1280x800; condition-based waits only.
- The test fails on the live build 7352d93f's code as a negative control.
- Evidence: frame-by-frame captures before (7352d93f) and after, with a per-frame JSON sidecar of child presence and elementFromPoint results; the usual matrix scenes 73 to 76 and the strip scenes, light and dark, phone and desktop; BUILD.txt in each folder.

# Notes

Owner UI request (fast-track, owner rule 286). Diagnose first with a failing test; fix on a branch from github/main; one owner gate; land only on the coordinator's LAND NOW.

## Resolution

Landed on main as d80a818f (gated as cde27ab1, owner gate green 2026-11-25 8:58 AM ET), deployed 9:04 AM ET 11-25, and the 15-minute deploy watch came back clean (9:04 to 9:19 AM ET: no app errors or panics, no restarts, no server errors on any route). The quoting view's
`drawSelectionChip` is now the one owner of the strip group: WTF? is shown and hidden with the row, Quoted reply and the X in
the same call, so all of them paint in one frame (out of the row, with no empty slot, where lookups cannot run); the glossary
view no longer decides WTF?'s visibility on selection change or popover close. The cause was a timing race, not z-order:
the glossary view's own selection-change listener hid WTF? synchronously before the strip's next-frame draw on a first
selection, and nothing showed it again until the next selection change, so the strip painted an empty slot where WTF? belongs.
Page tests step frames and assert the whole strip from its first frame (first selection on a fresh load, selection change,
re-selection, popover close, touch hold, keyboard open; phone and desktop) and fail on the previous page code. Side findings
filed as #2235 vibe-talk-wtf-popover-late-answer-reopens and #2236 vibe-talk-screenshots-discuss-config-scenes.
