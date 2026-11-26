---
title: 'vibe-talk-top-anchor-arrival-27px-race: an arrival at the foot intermittently moves the row being read 27.7 px in top_anchor_browser on phone-412'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- scroll-stability
- flaky-test
created_at: 2026-11-26T08:40:00-05:00
updated_at: 2026-11-26T08:40:00-05:00
---

# Description

`vibe-talk/tests/top_anchor_browser.py` (DAG step `vibe-talk.check.top-anchor-browser`, the `#2076` and `#2230 vibe-talk-reading-row-stays-put` walk) intermittently fails one of its two arrival steps at 412x915, always by exactly 27.7 px:

- `arrival_holds` (a live arrival with the reader at the foot, added 2026-11-25 in c8896041): "phone-412-full: a live arrival at the foot moved the reading row 27.7px over 32 frames", and the same on phone-412-windowed.
- `arrival_during_act` (an arrival landing while an expand settles at the end): "phone-412-full: an arrival during the act moved the touched top 27.7px over 32 frames".

It is a race already on main, not caused by the change under test where it was found. It first showed on the `#2285 vibe-talk-labels-browser-tapped-message-jumps` owner gate (tree d3b7a893), was cleared under the flake rule, and reproduces on main 8c6b111c. An instrumented copy of the `#2285` tree showed its new scroll code never runs in this walk (0 calls in 10 walks, while a chip tap in the same build triggered it at once).

Counts on 2026-11-26 (walk run alone, one at a time unless noted; load1 = host 1-minute load at the start of each run):

| Tree | Runs | Red | load1 |
|---|---:|---:|---|
| `#2285` tree d3b7a893, plain | 9 (3 + 5 alone, plus its gate) | 2 (gate: arrival at the foot, phone-412-full; local: arrival at the foot, phone-412-windowed at load1 91) | 85 to 107 locally |
| `#2285` tree, instrumented | 10 | 0 | 147 to 169 |
| main 8c6b111c | 15 | 0 | 119 to 175 |
| both trees at once (concurrent, a capture run) | 12 each | 1 each, in the same first run: arrival during the act, phone-412-full, 27.7 px | 134 to 170 |

None of these runs was hot by the rule (load1 at or above 237). The constant 27.7 px points at one element of that height appearing above, or disappearing from above, the row being read for a frame or more. A similar 27.7 px strip that a re-read used to show above the list on a label change was replaced by an in-place regroup in `#2279`, so a re-read or regroup path on arrival is the first suspect.

# Design

1. Name the element: run the walk with a recorder installed at each park at the foot, logging per frame the reading row's top, the scroller's place and size, the rows just above it and the scroller's children, plus every DOM mutation (child list, `hidden`, `style`, `class`), and dump the record on a red. Repeat on main until a red is captured.
2. Fix the page path that shows, hides or re-lays out that element so the row being read keeps its top through an arrival (owner rule 370): hold it through the scroll owner's existing holds, or keep the element's room stable, rather than loosening the walk.
3. If the walk itself races (for example it samples before the arrival's re-read has landed), make its waits condition-based on the landing, never a wall-clock delay (rule 346).

# Acceptance Criteria

- The element that moves the row by 27.7 px is named with a recorder trace from main.
- `top_anchor_browser.py` passes 30 runs in a row at 412x915 and 1280x800, windowed on and off, including runs with load1 at or above 237, with no 27.7 px move in either arrival step.
- The fix keeps the row being read still per frame (within 0.5 px, or 1.0 px where placement is on whole pixels) through an arrival at the foot and an arrival during an act, in a page test at phone and desktop sizes.

# Notes

Related: `#2230 vibe-talk-reading-row-stays-put` (the arrival-at-foot policy the walk checks), `#2076` (the top-anchor policy and its user-caused hold), `#2279 vibe-talk-post-facto-labels` (replaced a 27.7 px re-read strip with an in-place regroup), `#2285 vibe-talk-labels-browser-tapped-message-jumps` (the gate where it showed; its red was cleared under the flake rule with this issue as the named cause).
