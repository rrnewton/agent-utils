---
title: 'vibe-talk-archive-anchor-measured-once: one shared anchor measure per archive press'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- performance
- ui
created_at: 2026-11-21T06:10:00-05:00
updated_at: 2026-11-21T07:50:00-05:00
closed_at: 2026-11-21T07:50:00-05:00
---

# Description

[watermelon-full-preview] Fix 4 of the standing perf lane (#1976 vibe-talk-client-perf; coordb go 2026-11-21). Measured on the deployed build with the lane harness after #1990 landed (report /tmp/mu-scroll-report.md): an archive or unarchive still reads 169–221 layouts per press, almost none of it the dress itself. Attribution by caller: the scroll anchor is measured several times over — `holdingMessage` captures it (`captureScroll`) and then asks for the read position (`channelReadPosition`), which measures the same anchor again — and the unread-above measure (`measureUnreadAbove`, head and foot plus a binary search and a screen scan) runs once per render path in the press (54 layout reads of it at 1x).

# Design

- `holdingMessage` hands its captured mark to `channelReadPosition`, which reuses that mark's anchor row and top when it came from a message row (a jump's landed row, a filter-held row, or the scroll anchor itself), instead of measuring the anchor a second time. The search walk's chip anchor is not a row and is never reused this way. `wasAtNewest` and the thread-list surroundings are still read as today.
- Nothing further is held across passes for the unread-above measure. Two shapes for sharing it inside one press were tried and reverted on evidence: a per-frame hold and a per-turn hold both returned a not-yet-true answer inside a turn where the pinned page tests check the chip synchronously, hiding the way up (`#1167`) and double-reading a same-search redraw (`#249`/`#250`). Its existing held answer (`unreadHeld`) already answers passes whose key still matches with no layout reads, and that is where the remaining saving honestly ends.
- Nothing about what is drawn changes: the same rows are dressed (#1990), the same chips offered, from the same measures, taken once.

# Acceptance Criteria

- The #1990 archive event-count gate tightens: layout reads per archive/unarchive press are capped well under the 169–221 measured (target about half), still counting dressed rows and class writes per press, still event counts only (owner rule 346).
- Before/after numbers at 1x and 4x under the lane harness go to the budget file (via coordb): reads and long tasks per press, and the per-caller stacks.
- Existing Done/archive, scroll and jump checks green locally and in the gate; page-only change, owner-UI fast-track, combined checks on current main, screenshots for coordb.

# Notes

Owner-priority, fast-track. Standing lane mu-clientperf under coordb; one fix per issue, one commit per fix. Builds on #1990 vibe-talk-archive-single-row-dress. Follow-up after this: `o` fold height change.

## Resolution

[watermelon-full-preview] Dropped after measurement, 2026-11-21; no code landed. The holdingMessage anchor reuse saved nothing in aggregate: an archive or unarchive still reads 226 layouts per press at 1x and 225 at 4x on the lane fixture, with the same long tasks (2 at 1x, 4 at 4x). The one duplicated anchor measure is ~10 of those 226 reads; the rest of the place-keeping cost is spread across four independent captureScroll-style paths, each measuring its own anchor and neighbourhood positions, so a shared anchor alone cannot move the total. The unread-above sharing shapes were reverted on pinned-test evidence (`#1167`, `#249`, `#250`). The change itself was correct (all four page shards green with it alone) and was reverted uncommitted. A future fix would need to unify the position gathers of those four paths in one press, which is broader surgery than this issue scoped.
