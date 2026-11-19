---
title: 'wrkslots-rescue-partition-lows: three small private-history scan follow-ups'
status: open
priority: 3
issue_type: task
assignee: unassigned
labels:
- wrkslots
created_at: 2026-11-19T05:00:00.000000000+00:00
updated_at: 2026-11-19T05:00:00.000000000+00:00
---

# Description

Three LOW items deferred from the #1785 data-safety review (all fail closed today): (1) `_partition_kept_commits`' protected-root walk (`complete_protected_roots` in `py/wrkslots/cli.py`) builds its parent graph from the tip with no durable-store negatives, and `maximal_of` repeats a DFS per candidate pair — rare path, bounded by maximal protected commits, but wasteful with very large rescue inventories; limit the walk by the same negatives the scan used and compute maximality once. (2) Dropped protected commits returned by that walk carry ref "" in the receipt's dropped list; use the naming ref from the scan's history.refs so each dropped commit names the ref that held it. (3) The R3e deferred-malformed-reflog fix over-refuses a malformed reflog line naming a durable-held commit (one a durable store already holds, so nothing would be lost); clear the problem when the commit is held by a durable store, not only when kept by reachability.

# Design

Small, scan-local changes in `py/wrkslots/cli.py`, each with a deterministic regression test in the style of `py/wrkslots/tests/test_review1785_regressions.py`.

# Acceptance Criteria

Each item has a failing-before/passing-after test; the reviewer regression suite stays green; no persisted field changes.

# Notes
