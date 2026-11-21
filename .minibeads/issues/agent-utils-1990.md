---
title: 'vibe-talk-archive-single-row-dress: archive and unarchive dress only the acted rows and their neighbours'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- performance
- ui
created_at: 2026-11-20T15:10:00-05:00
updated_at: 2026-11-21T01:40:00-05:00
closed_at: 2026-11-21T01:40:00-05:00
---

# Description

[watermelon-full-preview] Fix 2 of the standing perf lane (#1976 vibe-talk-client-perf; coordb go 2026-11-20, plan approved with the windowed addition). Measured at aee56a3c on an 816-row / 21,587-node fixture (devbig030): one archive or unarchive (`e`) took 356 ms at 1x and 1,902 ms at 4x CPU throttling, with 3,340 mutations, 1,634 class writes and ~270 layout reads — because `renderDoneChange` dresses every row on screen (`renderChannelRows`) for a one-row change.

# Design

- On the threaded path where rows stay put (not Hide read, not the Threads view), a Done or put back dresses only: the mounted rows holding the acted messages (in the channel list, the pinned list and copies unpacked under cards), the mounted root row of each acted message's thread (its unread chip follows the tally), and the rows immediately beside an acted row (read-run and arrow neighbours).
- Place is kept exactly as today by the existing holders (`holdingMessage`, `holdingReplyArrows`) around that subset; no row is measured except through those holders and the label measure of a dressed row. Read runs are re-derived as today (they span neighbours); the side thread card and the open thread's card are repainted from the same tally. Everything else the full pass does (author census, ordering, seams, pinned membership, arrows structure) does not change with an archive and is not redone.
- The ids changed travel from the act (`dismissMessages`, `restoreMessages`, their refusal take-backs, and `mirrorArchived` in the other column) into `renderDoneChange(ids)`; any caller without ids keeps the full pass, as do Hide read and the Threads view.
- coordination: mu-sendflash (#1983) and mu-windowed (#1985) both confirmed no collision; a send's or the window renderer's settle/anchor code is not touched.
- With windowed lists On, an acted message whose row is not currently mounted (archived from the keyboard while just off-screen) dresses nothing and mounts nothing: the state is already in the store, the row is dressed from it when the window next mounts it, and neither the acted row nor the whole list is measured.

# Acceptance Criteria

- Event-count gate (browser, owner rule 346, no wall-time thresholds): one `e` archive and one unarchive over the big fixture dress at most 20 rows, with at most 50 layout reads and at most 100 class writes per press; with windowed lists On, archiving an off-screen (unmounted) row mounts 0 rows and reads no whole-list layout. Before/after numbers at 1x and 4x under the Phase 1 harness go to the budget file (via coordb).
- The acted row shows Done/unDone at once, its thread root's unread chip and the side/open thread cards follow, a refused write puts the acted rows back, and two-column mirroring still draws the act in the other column.
- Existing Done/archive suites green (JS view/page, done-feedback and related browser checks where a browser runs); page-only change, owner-UI fast-track, path-selected gate on devbig014 plus before/after screenshots from devbig030 for coordb.

# Notes

Owner-priority, fast-track. Standing lane mu-clientperf under coordb; one fix per issue, one commit per fix. Builds on the #1976 Phase 1 measurements and #1984's changed-only writes. A candidate follow-up (not this fix): mu-windowed's `refreshThreadDirectory` full dress pass once per directory read.

## Resolution

[watermelon-full-preview] Landed on github/main as 441f9c71, deployed 2026-11-21 1:29 AM ET. Archive and unarchive dress only the acted rows, neighbours, thread roots and copies; per press, DOM mutations 3,343 -> 69 and class writes 1,634 -> 2 on the lane fixture, archive 356 -> 184 ms at 1x and 1,902 -> 1,129 ms at 4x (report-only numbers; the gate counts events).
