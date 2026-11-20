---
title: 'vibe-talk-search-typing-changed-only: search typing writes only changed rows and settles in one coalesced pass'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- search
- ui
- performance
created_at: 2026-11-20T14:10:00-05:00
updated_at: 2026-11-20T15:05:00-05:00
closed_at: 2026-11-20T15:05:00-05:00
---

# Description

[watermelon-full-preview] Fix 1 of the standing perf lane (#1976 vibe-talk-client-perf, Phase 1 report /tmp/mu-clientperf-report.md, coordb go 2026-11-20). Measured at aee56a3c on an 816-row / 21,587-node fixture (devbig030): search key input-to-paint p50 79 ms at 1x and 1,744 ms at 4x CPU throttling. The #1913 fast pass still writes the `search-hidden` class on every row of every search list on every keystroke (816 class mutations per key, forcing whole-document style recalculation), and the 120 ms mark timer and 150 ms full-search timer land between keys whenever typing is slower than ~7 keys/s — at 4x, on essentially every key, each landed pass writing ~5,712 class mutations. 0 fetch and 0 storage writes per fast key; the server ask is debounced and stays that way.

# Design

- The fast pass (`applySearchFast`) and the full pass (`applySearch`) write `search-hidden` only on rows whose visibility actually changed, tracked by a per-row last-state bit (`searchHidden` on the row, reconciled with the class the row actually wears so another writer cannot leave it stale).
- `setClassWord` (view/dom) is a no-op when the word already stands as asked, so every caller stops paying a DOM write for an unchanged class.
- Mark and full passes coalesce into one settled pass on the typing path: over a long list the deferred pass first takes the typed words as the marked words, then runs the one full filter change; the separate 120 ms mark timer no longer runs per keystroke there. Short lists keep their synchronous landing and mark timer. Enter still marks at once.
- The settled pass waits for a real pause, adaptively: its delay is at least the existing 150 ms floor and grows with the measured gap between recent keys (capped), so a throttled phone whose keys arrive seconds apart does not pay a full pass between keys. The server ask stays inside the settled pass, debounced exactly as today.
- Search results and marks stay exactly as today after the pause: hits appear, disappear on refine/backspace, and are marked from the settled words.

# Acceptance Criteria

- Event-count gate (browser, owner rule 346, no wall-time thresholds): typing a query over the big fixture writes `search-hidden` on no more than about 5% of loaded rows per keystroke, with 0 fetch and 0 storage writes per key, and no full pass lands between keys typed at a steady pace (the settled count/marks land once after the pause).
- Before/after numbers under the Phase 1 harness at 1x and 4x in ai_docs/reports/measurements/vibe-talk-perf-budget.md (via coordb): key latency p50/p95 and per-key class mutations, layout reads, fetch and storage counts.
- Hits appear, disappear and re-mark correctly after fast typing and backspacing; settled rows, counts, empty sentence, seams and marks equal the pre-fix build for the same queries.
- Existing search suites green (search-typing-fast browser gate, JS view/page search tests, focused validation); page-only change, owner-UI fast-track path-selected gate; screenshots via devbig030 with BUILD.txt for coordb review before deploy.

# Notes

Owner-priority, fast-track. Standing lane mu-clientperf under coordb; one fix per issue, one commit per fix. Builds on #1913 vibe-talk-search-typing-lag and the #1976 Phase 1 measurements. Fix 2 (archive single-row dress) gets its own number after this lands.

## Resolution

[watermelon-full-preview] Landed on github/main as 260d5b0b (replayed tip 353412fc with the #1976 issue file), deployed 2026-11-20 2:54 PM ET. Changed-only search writes and one coalesced adaptive settled pass; browser gate counts class/fetch/storage writes per key. Before/after (816-row fixture, devbig030): refining-key class writes 816 -> 0-5, settled-pass class writes 5,713 -> 1; search latency p50 79 -> 78 ms at 1x, 1,744 -> 1,413 ms at 4x.
