---
title: 'vibe-talk-read-strip-foot-reader-moves: on a phone the message being read moves when the read strip appears or goes, for a reader pinned to the list''s end'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
- read-aloud
created_at: 2026-11-26T23:50:00-05:00
updated_at: 2026-11-26T23:50:00-05:00
---

# Description

Found by the advisory design review of #2347 vibe-talk-read-strip-foot-same-frame (read-aloud-blocks-browser captures at 393x852, light and dark). When a read starts on a phone and the read strip appears, the list's frame changes height (the bar's second row goes, the strip takes its place: the list's height goes from 703 to 656 px), and a reader whose list stands at its end stays pinned to the end: the message being read moves up by 47 px (its card top from y=81 stopped to y=34 playing and paused), and back when the read stops. At 412x915, where the content fits the screen, the card holds still (y=86); at 375x812 the read card's header is pushed into the top fade. Owner request 370 / R12: nothing may move the row being read. The liaison made it owner-UI priority.

Most likely pre-existing, not #2347's: the foot keeper's decision is the same function as before (`measureReadSteer`, the read strip's ResizeObserver body: `chipsFollowFoot` with the strip's growth, then `toFoot`), which #2347 only runs in the task that shows the strip instead of at the next rendering; the unfixed run went red before its 393 captures, so a same-size before/after pair on main was not captured.

# Design

- When the read strip appears or leaves, the message being read keeps its top: the list keeps its scroll position (or holds the reading row through the scroll owner) instead of re-pinning to the list's end, and the end slips below the strip, reachable by scrolling.
- A reader truly following the newest line during a live arrival is out of scope here; the strip's coming and going is a frame change, not an arrival.
- Shares the foot-keeper code with #2346 vibe-talk-search-landed-reader-follows-foot; land in order after it.

# Acceptance Criteria

- A per-frame trace at 393x852 and 375x812 (light and dark) of a read started and stopped with the list at its end: the read row's top moves <= 0.5 px across the strip appearing and leaving; main's 47 px as the negative control, with a JSON sidecar.
- read-aloud-blocks-browser stays green (its one-shot phone layout check: the list ends at the strip's top).
- Shots of stopped, playing and paused at 393 and 375 with BUILD.txt, and the advisory design review.

# Notes

Owner-UI (rule 370), filed at the coordinator's request from #2347's design review (issue file only). Related: #2347, #2346, #2230 vibe-talk-reading-row-stays-put, #1185 chip-height-snap.
