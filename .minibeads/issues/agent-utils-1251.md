---
title: 'scroll-single-owner: one owner decides every scroll write, and the 60 s soak gates in strict mode'
status: open
priority: 0
issue_type: task
assignee: mu-renderloop
labels:
- vibe-talk
- ui-stability
created_at: 2026-11-11T00:30:00+00:00
updated_at: 2026-11-11T00:30:00+00:00
---

# Description

[opus 5.5] Part of #1210 ui-stability-no-async-shift (stage D4, render-loop adoption). The owner asked tonight (reply 352) for one owner of scroll decisions. Today several page writers set the scroll position on their own, and the browser's native scroll anchoring corrects as well. So one frame can carry two or three competing corrections, and the reader sees the row move. The target model is in the harness report "vibe-talk redraw model: one render loop, one scroll owner" (2026-11-10): one render loop per frame, one owner of the scroll position.

This issue delivers the single scroll owner end to end, together with the D4 render-loop adoption that #1210 sequences after the soak, D1 freshness and the D2 summaries batching.

# Design

- a. Guard check in the page tests: fail on any direct `scrollTop`, `scrollTo`, `scrollBy` or `scrollIntoView` write outside the loop's scroll owner (scrollkeep). An explicit allow-list covers user-initiated jumps routed through the owner (for example the mention jump of #1224 mention-jump-in-view). It lands FIRST in report mode, listing today's writers, and becomes enforcing for each path as that path moves into the loop.
- b. `overflow-anchor: none` on both scrollers, so the loop's manual anchor is the only correction per frame.
- c. The loop owns the per-act anchor choice: the first on-screen row; the foot when following; and the collapse rule of #1215 collapse-anchor-below.
- d. The 60 s stability soak from #1210 runs in `--strict` as a gate at phone and desktop widths.
- Also in scope (D4): adopt the render loop path by path. This includes the reaction-overlay decide-before-paint, which the D2 summaries work does not cover. Each path names its functions to the coordinator before it is edited. The chip observers and selection survival stay with their current owner.
- Every check judges drawn state and geometry, never elapsed time or frame counts. It must pass on a host more than 75% busy (owner rule 346).

# Acceptance Criteria

- The guard check exists, first in report mode (listing every direct scroll writer), then enforcing with only the scroll owner and the allow-listed user-initiated jumps left.
- Both scrollers carry `overflow-anchor: none`, and the soak shows no frame with more than one scroll correction.
- The per-act anchor choice lives in the loop owner, covering first-row, follow-foot and #1215's collapse rule.
- The 60 s soak passes in `--strict` at phone and desktop widths and is part of default validation.

# Notes

Filed by the devbig030 coordinator at the lead's request (owner reply 352). Lane: mu-renderloop.

[watermelon-full-preview] (b) is per-scroller, on A/B evidence (2026-11-10): 10 runs per step per tree in one CPU scope (AllowedCPUs=240-243, burners in-scope), base = github/main, ours = both-scrollers-off plus (c). keyboard_hides_bar 10/10 vs 0/10; reply_continuity 10/10 vs 0/10; approval_card small-phones 10/10 vs 1/10. Native anchoring was silently correcting #scroll-area foot/place holders (keepers, composer keyboard, reply send) that the owner does not route yet, so #scroll-area keeps anchoring until P1+P7 land; #side-scroll is off now, and (c) the collapse anchor choice is in the owner. Re-adding #scroll-area is its own landing with those three steps as proof.
