---
title: 'vibe-talk-scroll-style-recalc: the rows-below recount runs once a scroll settles, not once a frame'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- scroll
- ui
- performance
created_at: 2026-11-21T09:00:00-05:00
updated_at: 2026-11-21T09:00:00-05:00
---

# Description

[watermelon-full-preview] Fix 5 of the standing perf lane (#1976 vibe-talk-client-perf; coordb go 2026-11-21 on the measure-only step). #2034 took the layout reads out of scrolling, but the stalls stayed: at 4x CPU throttling a PageDown screen still pays 2–3 long tasks of 85–195 ms. An instrumented run on the same 816-row fixture named the writer: `scheduleBelow`'s frame callback (`view/channel/folding.ts`), asked from `belowForJump` under the scroll listener, cost about 140 ms a callback at 4x (1,127 ms across 8 callbacks in 4 screenfuls), because every animation frame of a live scroll recounted the rows below and redrew the jump. Timeline shares over a six-screen scroll: UpdateLayoutTree 1,530 ms, FireAnimationFrame 1,576 ms, actual Layout 53 ms — style and script inside the frame, not layout reads.

# Design

- The recount behind the jump label stays per-frame for a young burst, exactly as today — a burst inside one frame, a click, an arrival still settles in its own frame, and the pinned `#261`-era test "the count is remade at most once a frame while the reader scrolls" passes unmodified. Once asks have arrived in a fourth frame window running — a scroll well under way — asks defer the recount to one settle countdown (BELOW_SETTLE_MS, 150 ms) armed behind them, and later windows only rearm it; the countdown recounts once when the scroll stops (a scroll that outlasts a countdown repeats at that cadence, never per frame). (Two other shapes were tried and reverted on pinned-test evidence: a settle keyed to wall-clock quiet, whose countdown the fake-clock fixtures could never fire, leaving the jump count stale in browser walks; and a two-window deferral, which deferred the test's own two-window settle.)
- The jump's hidden/shown state is untouched, and correctness comes from the countdown rather than frame cadence, so a stopped scroll shows the true count when it stops. Non-scroll changes that move the count recount as today (interval elapsed, key changed, a direct render call).

# Acceptance Criteria

- Event-count gate: the scroll browser gate caps jump-offer paints per screenful of scrolling (style-affecting attribute writes over the same scroll are capped too); counts only, never wall time (owner rule 346).
- Before/after at 1x and 4x (the lane harness): layout reads, long-task count and max, UpdateLayoutTree totals, FireAnimationFrame totals, wall per screenful.
- All page shards, scroll, documents keep-place, one-page-complete, view-switch and done-sync checks green. Page-only change, owner-UI fast track.

# Notes

Owner-priority, fast-track. Standing lane mu-clientperf under coordb; one fix per issue, one commit per fix. Follow-up candidate (b), containing style recalc scope (content-visibility/containment on rows, the :has()-heavy CSS), stays parked for coordb's separate go.
