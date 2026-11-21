---
title: 'vibe-talk-scroll-layout-reads: scrolling a loaded channel reads the screen, not the whole list'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- scroll
- ui
- performance
created_at: 2026-11-21T03:00:00-05:00
updated_at: 2026-11-21T03:00:00-05:00
---

# Description

[watermelon-full-preview] Fix 3 of the standing perf lane (#1976 vibe-talk-client-perf; coordb go 2026-11-21 on the measure-only report /tmp/mu-scroll-report.md). Measured on the deployed build over an 816-row fixture with the lane harness: one PageDown screenful costs 4,960–6,002 getBoundingClientRect reads and a 50–62 ms long task at 1x (frames p95 300 ms at 4x). Attribution: `firstRowInView` filters every loaded row, and each `rowInView` test re-measures the pane's head (`floatingClearance`, 3 reads) and foot (area + floating rooms) — ~2,451 clearance reads and ~4,900 reads in total from that one scan; the list's scroll listener also runs its whole pass on every scroll event.

# Design

- `firstRowInView` (view/shell/keyselection) measures the pane's head and foot once, then binary-searches the rows' bottoms for the visible band and scans only that band — the same neighbourhood measure the search path uses (#1913) — instead of testing every loaded row. Semantics unchanged: first wholly-shown row, else the first partly shown, else none.
- The pane's head (`floatingClearance` through keyboard's pane) leaves the per-row path with it: measured once per first-row search instead of once per row tested, which is where its ~2,451 reads a screenful came from. (Two stronger shapes were tried and reverted on evidence: a frame-held clearance cache and a frame-coalesced scroll listener both break the page's own pinned source shapes and frame accounting — `threads.mjs`' chips needle and the `#261`/`#226` one-frame-for-a-burst count — while adding no saving beyond this one.)
- Windowed rows are untouched: the search reads only mounted rows' boxes as today, and the Documents keep-place (#2003) and reader-position (#2005) paths keep their measures, covered by their browser checks.

# Acceptance Criteria

- Event-count gate (browser, owner rule 346, no wall-time thresholds): a PageDown screenful over the big fixture reads at most a few hundred layouts (down from ~5,000), with the scroll still landing, the jump/read tools drawn, and older loading still triggered at the top.
- Before/after numbers at 1x and 4x under the lane harness go to the budget file (via coordb): layout reads and long tasks per screenful, frames over 16 ms.
- Existing scroll, selection, Documents keep-place (#2003) and reader-position (#2005) checks green, locally and in the gate; page-only change, owner-UI fast-track, path-selected gate, screenshots for coordb review.

# Notes

Owner-priority, fast-track. Standing lane mu-clientperf under coordb; one fix per issue, one commit per fix. Follow-ups in order after this: archive shared anchor measure, then `o` fold height change, each on its own issue.
