---
title: 'search-typing-foot-jump: once in a gate, typing a search moved the phone row being read 110 px at the foot (unreproduced)'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- search
created_at: 2026-11-28T08:11:00-05:00
updated_at: 2026-11-28T08:11:00-05:00
---

# Description

In the owner-priority gate of `#2445` search-clear-two-columns-found-hold (head 7921b0a0 on main 9bf112c0, 2026-11-28 about 7:5x AM ET, load1 300-350), `vibe-talk.check.search-typing-stays-browser` (`#2424` search-view-unstable-while-searching) failed once:

- `light-phone-keyboard-big-foot-slow`, 222 frames: frame 28 (query `need`), the row being read (message 392) moved +110 px on a clamp-bound frame (limit 1.0 px). Clamp-bound moving frames [28, 79]; one hand-over to the next row. The other 21 walks passed (max 0.41 px).

The step log kept only the summary line, so nothing says what moved. `#2445` changes nothing on a phone (its CSS is scoped to two columns; its place-keeper change removes a two-column skip only), so the phone frame runs main's code.

Not reproduced (fixture server; load1 recorded per run):

- that walk alone, alternating: main 9bf112c0 5/5 and 7921b0a0 5/5 pass (load1 317-357); the worst clamp-bound move is 0.25 px, always one frame, at the final settle (about frame 77-85).
- main pinned to 2 CPUs: 4/4 pass (load1 266-278). Chromium CPU throttling 4x and 8x: pass. A 500 ms pause after `need` (the settled pass and its server ask mid-typing): pass, the list grew 596 px at that settle and the row held 0 px.
- the full 22-walk check: main pass (load1 309); another lane's post-deploy run of main: pass (210 frames, 0.25 px).

On a passing trace the walk stands at the list's foot throughout (`scrollHeight - clientHeight - scrollTop` = 0, so every frame is clamp-bound); the fast pass hides rows at `n`..`ne` (35664 -> 3728 px) and hands over 391 -> 392; `nee`, `need` and `needl` move nothing. The first key also adds about 105 px above the rows (height +105, scrollTop +106, compensated in the same frame).

# Design

- Hypothesis: about 110 px of room changed at the foot without the scroll following, either the first key's ~105 px above the rows added again (or late) with no matching scroll, or a foot element of that height (the place-keeper's foot room, the search's foot line) going while the list stood clamped at its foot.
- The walk prints its own trace on failure (this issue's first code commit, gated on its own): every frame where the scroll, the list's padding above and below its rows (head and foot room), what stands above the list and past its foot, the viewport or the row being read changed, with failing frames marked, so the next occurrence names its cause in the gate's step log.
- Fix the path the trace names; until then the walk stays as it is (it caught a real 110 px move once).

# Acceptance Criteria

- The typing walk's failure output carries the per-frame digest (shown with a forced failure).
- When it recurs: the cause named from the digest, a fix that holds the row on that path, and a walk that fails on main before the fix.

# Notes

- Found in the `#2445` owner gate; the table above is from 2026-11-28 7:5x-8:0x AM ET.
- The walk's failure digest, shown with a forced failure (its limits set to 0) on main: the ~105 px the first key adds is content above the list (`logTop` 214 -> 320, the scroll following in the same frame), not the list's own head room (`padTop` stays 0); the hold's foot room shows as `padBottom` (103.75 px at the final settle). So the gate's +110 px at `need` fits that content above the list changing without the scroll following, more than a foot element going.
- The same gate's other load reds (fold-steps, reload-place, view-switch) and this one all passed on its solo rerun (load1 about 127).
