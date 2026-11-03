---
title: 'merge-queue-stats-backfill: reconstruct the last day''s queue metrics from existing state'
status: open
priority: 1
issue_type: feature
assignee: cl-mq@devbig030
labels:
- merge-queue
created_at: 2026-11-03T16:12:59.063422232+00:00
updated_at: 2026-11-03T16:12:59.063422232+00:00
---

# Description

[opus 5.5] Owner request (2026-11-03), part of the merge-queue instrumentation (#822): a consumer's report on Wednesday 07:30 ET needs real numbers for the last 24 hours, before the decision log existed.

# Design

- A one-shot `merge-queue stats --reconstruct` (or a report) rebuilds what it can of the #822 metrics from the existing queue state: plans, results, events and git history.
- It states explicitly which fields and metrics cannot be reconstructed and why.

# Acceptance Criteria

- Running it on a queue state directory prints the reconstructable metrics for a window and a list of what is missing.
- A test reconstructs metrics from a fixture queue history.
