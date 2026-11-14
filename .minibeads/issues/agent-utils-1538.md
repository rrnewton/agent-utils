---
title: 'fold-peel-five-at-a-time: peel a folded read run five at a time from either edge'
status: closed
priority: 0
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-14T16:14:54.000000000+00:00
updated_at: 2026-11-14T16:14:54.000000000+00:00
closed_at: 2026-11-14T16:14:54.000000000+00:00
---

# Description

[watermelon-full-preview] Owner ask on 2026-11-13: a long folded run of read messages is all-or-nothing — opening it shows everything at once. The owner asked to peel it a few at a time: grey (first 5) / (last 5) on folds of ten or more, repeatable, and everything that comes out of a fold starts collapsed.

# Design

- A folded run of ten or more read messages offers grey (first 5) and (last 5) buttons at the far end of its line.
- Each peel reveals five messages from that edge, collapsed, while the fold line stays with its count reduced; peels repeat, and under ten the peel buttons go and the line opens the rest, also collapsed.
- The same peel applies wherever folded read runs are drawn with shared code: the channel lists, threads unpacked under cards, and the Threads cards' runs.

# Acceptance Criteria

- A fold of ten or more shows (first 5) and (last 5); each peel reveals five collapsed messages from that edge and reduces the fold's count.
- Repeated peels keep shrinking the fold; below ten the peel buttons disappear and the line opens the remainder collapsed.
- Peeled messages start collapsed in every list that shares the fold code.

# Notes

Owner ask of 2026-11-13 (audit rows 59-60). Number assigned for this reconcile; no tracker issue existed before.

## Resolution

[watermelon-full-preview] Landed in fe9a8aa356e77c0a6904d2b665091b6b21b3f7c3 on main and live in release ba329b5a. Folded read runs of ten or more peel five at a time from either edge, repeatedly, with everything out of a fold starting collapsed. Verified as an ancestor of the live release.
