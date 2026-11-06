---
title: 'merge-queue-conflict-scan-large-range: the landing conflict-marker scan reads every commit in base..tip'
status: open
priority: 3
issue_type: task
created_at: 2026-11-06T04:30:00.000000000+00:00
updated_at: 2026-11-06T04:30:00.000000000+00:00
---

# Description

`conflict_markers_in_range` greps every commit in base..tip for conflict markers (Cargo.lock and ci/dag/*.json only). For an exact merge landed with `submit --exact` (#922 merge-queue-exact-merge-head), that range is the whole imported history. That is correct, but a very large import could make the landing guard slow.

# Design

Measure first on a large synthetic import. If it matters, check only what the landing changes relative to the base (the tree diff base..tip, plus the merge commit itself), or scan only first-parent commits for exact merges. Keep the guard's coverage for ordinary replayed changes.

# Acceptance Criteria

- A benchmark or test with a merge that brings in thousands of commits shows the guard's cost, and any change keeps the existing conflict-marker tests passing.

# Notes

Found by cl-sync while landing #922 (2026-11-05 11:25 PM ET). Low priority; the lead asked that no work start on it yet.
