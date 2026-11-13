---
title: 'scroll-single-owner: one owner decides every scroll write, and the 60 s soak gates in strict mode'
status: closed
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

[watermelon-full-preview] (b) completed for #scroll-area (the anchoring-off flip), after P1, P7/P7b and the reply-send landing routed its holders through the owner: `overflow-anchor: none` on #scroll-area joins #side-scroll, a view switch restores the channel by the owner's anchor mark rather than the saved number, and a view or layout switch re-takes Main's foot note before it can move the reader (the chips keeper took a reader 40px up Main to the foot on a note latched from the thread-list layout's clamped foot; traced, then fixed in the keepers' context). Flipped proof in one CPU scope (AllowedCPUs=240-243, burners in-scope), 10 runs each: keyboard 10/10; reply_continuity 10/10; approval_card small-phones 10/10; approval_card phone-threads 10/10 (was a deterministic 40px move). Remaining: D4 paths by path, then (d) the 60s --strict soak with #1250's stability rework.

## Resolution

[watermelon-full-preview] Delivered, in order: the report-mode scroll-writer guard (7b7535ab); (b)+(c) per-scroller anchoring with the collapse anchor choice in the owner; P1 keepers/footroom, P7/P7b composer/keyboard, the reply-send landing through the owner; #1215 collapse-anchor-below; the anchoring-off flip for #scroll-area (8b11d4b3); D4 path by path — P2 placement completed with the deep-link landing, P9 reaction-overlay decide-before-paint, P3 collapse/fold, P5 shell keys, P6 user jumps (with the dom focus reveal through a ruled boot handoff), P8 legacy script, P4 documents; and (d), the 60 s soak gating in --strict with #1250's behaviour-only scroll-stability rework re-enabled (78c0288f). The guard now enforces every path (P1, P2, P3, P4, P5, P6, P7, P8, P9): no direct scrollTop/scrollTo/scrollBy/scrollIntoView write stands outside scrollkeep but the two allow-listed non-writers (a stored record field; the dom layer's pre-boot reveal). One owner decides every scroll write; native anchoring is off on both scrollers; the per-act anchor choice (first on-screen row, the foot when following, #1215's collapse rule) lives in the owner.
