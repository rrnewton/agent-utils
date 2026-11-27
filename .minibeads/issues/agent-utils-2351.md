---
title: 'vibe-talk-read-strip-foot-reader-moves: on a phone the message being read moves when the read strip appears or goes, for a reader pinned to the list''s end'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
- read-aloud
created_at: 2026-11-26T23:50:00-05:00
updated_at: 2026-11-27T06:50:00-05:00
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

## Resolution

[opus 5.5, cl-signpost3] Landed 39c0d887 (gated head 48d31adf, rebased onto main 1347494d after #2354 landed as 99990873). In a channel list the read strip's measure (measureReadSteer) no longer takes a reader at the list's end to the new foot when the strip's size changes. The list keeps its scroll position, the row being read keeps its top, and the end slips below the strip, offered by the jump, as #2354 holds the other growth the reader did not cause. The call transcript follows its foot as before (#431).

Evidence (page builds of 48d31adf and base 1347494d):
- Per-frame traces at 393x852 and 375x812, light and dark: 0 px on the frame the strip appears. Base moved the row 47 px each way (the 375 header went under the top fade).
- read_aloud_blocks_browser 3/3 green.
- Shots: step 12 playing, paused and stopped; keyboard really open; desk 1280 light and dark. The liaison accepted the matrix without a Discuss-open cell (the harness cannot click Read on the desk profile: #2370).
- Page test: 0 px, with a negative control (+47 px). Full node suite 3492/3492.

Gate:
- The full owner gate on 48d31adf failed on python.wrkslots shard-2 and shard-4 (known red, #2372) and on vibe-talk.check.reload-place-browser.
- reload-place was 13/13 on each tree in --detach probes. A ResizeObserver probe showed the strip's code never runs in that check. One exclusive owner rerun of the step passed (journal clean, no NO_RESULT). The drift is tracked as #2373.

Still moving, by design or tracked:
- (a) The read-follow's centring at a read's start (#1587/#1588) still moves the row 14-37 px. That is an owner question in #2367.
- (b) The 32 px clamp drop at a read's stop at 393 is fixed in #2367's own commit.

Desk (liaison): keeping the reader's row is rule 370. The ~50 px of list end under the floating desk strip is the #2314/R18 class, and gets that band in a follow-up if the owner approves R18/B1.

The design review's R10 finding (the Newest chip's 12 px phone inset) is #2368; its pre-existing findings stay out of this issue.
