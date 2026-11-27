---
title: 'vibe-talk-thread-search-park-first-pass: a thread search parks its first result below the chip panel on the first settled pass'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- search
- owner-ui
created_at: 2026-11-25T15:45:00-05:00
updated_at: 2026-11-26T19:56:00-05:00
closed_at: 2026-11-26T19:56:00-05:00
---

# Description

Found while tracing #2251 vibe-talk-search-scope (scene "search scope thread"), and on main frame for frame: on a phone (412x915), searching inside a thread view leaves the thread's first result UNDER the search chip panel (result top 180 px, panel bottom about 305 px) after the first settled pass. The filter place-keeper puts it back there. About 400 ms later the redraw for the server's count runs #2200's thread-view parking and moves the result below the panel (to about 311 px). So the reader sees the first result covered, then a jump of about 131 px. Owner request 370: nothing may move the row being read on its own.

# Design

- #2200's thread-view parking runs at the first settled pass that leaves the first result under the chip panel, and the filter place-keeper keeps the parked place rather than restoring the covered one.
- The later count redraw then finds the result already parked and moves nothing.
- Shares the head room and parking code with #2256 vibe-talk-search-head-room-no-shift and #2257 vibe-talk-search-fast-pass-holds-reader, so it stacks on them in landing order.
- Constant layout reads per pass (the panel bottom and the first result), never one per row.

# Acceptance Criteria

- A per-frame trace (rAF-stepped) of the thread's first result from the first key through 30 frames after the server's answer, phone 412x915 and desktop 1280x800, light and dark: the result is never under the chip panel after the first settled pass, and moves 0 px at the answer; main's 180 -> 311 move at about 400 ms as the negative control, with a JSON sidecar.
- A page test: a thread search whose first settled pass leaves the first result under the chip panel parks it below the panel in that pass, and the count redraw leaves it in place.
- The #2200 and #2251 search-scope scenes stay green.

# Notes

Owner-UI (owner 370), small follow-up asked for by the owner side on 2026-11-25. Goes to the lane that owns #2256 and #2257, after #2257. Related: #2200 (thread-view parking), #2251 vibe-talk-search-scope, #2230 vibe-talk-reading-row-stays-put.

Stack notes (from #2262 (b) tracing, 2026-11-25, tip page build ec27ce01; scroll-writer probes at desk and phone size):
- desk 1280x800, lantern thread view, reader at scrollTop 0, key 'lantern': at the key, placeFilterAnchor nudge 0 -> 59 (head row kept, 0.2 px); at the answer, renderServerSearch preservingScroll writeHold 59 -> 125 (server hit row 151 inserted above the head row; row held), then renderSearchCount (refreshSearchCount <- renderServerSearch) intoView of the current hit (row 151) -> 49: head row +75.9 px.
- phone 412x915 (1 in ~3 runs): applySearch -> renderSearchCount intoView 106 -> 96, markSearchNow redraw preservingScroll restores 96 -> 106 (same task); at the server ask, refreshSearchCount <- renderServerSearch intoView -> 96, scroll event painted at 96 (one frame, +9.6 px) before the answer's hold 96 -> 162 and intoView -> 68.
- Fix landed here: park only when the first result's rect straddles the panel's bottom, and only once per search and row (parkedFor keyed by search key + row id, cleared when the search closes).

## Resolution

[opus 5.5] Landed in b41730eb (2026-11-26 7:54 PM ET), the liaison's LAND NOW. renderSearchCount's thread-view parking now acts only when the first shown result straddles the chip panel's bottom, and only once per search and row (parkedFor keyed by search key + row id, cleared when the search closes), so a server hit inserted above the reader no longer moves the row being read at the answer, and the one-frame re-park at the server ask is gone.

Evidence (liaison-approved on the first head, re-checked on the landed tree): per-frame rAF trace of the head reader through 30 frames past the answer, lantern thread view, phone 412x915 and desktop 1280x800, light and dark: worst move 0.5 px phone / 0.2 px desktop (whole-pixel placement, threshold 1.0 px), first result never left under the panel (straddle 0); main moved the same row 103.5 px phone / 76.1 px desktop (negative control). The rebased tree's trace and shots matched the approved ones (phone shots pixel-identical, desktop differing only in anti-aliasing). Page test: park_walk in search_typing_fast_browser.py (fails on main's searchrows.ts with "head row moved 103.5 px").

Gate history: the first head's owner gate went red on search-typing-fast-browser because the new walk's probe was named PARK_JS and shadowed the head-room walk's placement probe; renamed THREAD_PARK_JS, and the head-room walk now refuses a non-string row id (the suite then passed 5/5 alone on the tip and on main). The landed head's owner gate was red only on vibe-talk.shots.screenshots-desks (scene 82-agent-replies-jump timing out, #2175); its exclusive rerun passed. Under the liaison's screenshot flake rule: at load1 under 237 throughout, scene 82 passed 13 of 14 walks on the tip (alone 5/5, full desk walk 5/5, plus the rerun and two step runs) and 10 of 10 on unmodified main in the same alternating runs; scene 82 never opens the search, so it cannot reach the changed branch. Scene-82 rate recorded on #2175.
