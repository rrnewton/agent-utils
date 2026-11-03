---
title: 'merge-queue-decision-log: one record per planned validate, and merge-queue stats'
status: closed
priority: 1
issue_type: feature
assignee: cl-mq@devbig030
labels:
- merge-queue
created_at: 2026-11-03T16:12:59.051433942+00:00
updated_at: 2026-11-03T16:26:34.854593254+00:00
closed_at: 2026-11-03T16:26:34.854592964+00:00
---

# Description

[opus 5.5] Owner request (2026-11-03): "As long as we have limited validate slots, we're always making choices about which speculative validates to run. I want to make sure we're tracking those choices and have clear metrics we can seek to optimize." Today the queue keeps plans, results and events, but no per-validate decision record and no throughput metrics.

# Design

- Decision log: one JSONL record per planned validate, in its own subdirectory of the queue state, rotated with a size or age cap. Fields: candidate set (changes and SHAs), base, batch size, speculation depth, predicted pass probability (null until priors exist), per-change features (#823), the validator (local or donor host), cost (CPU and wall), outcome, the culprit change for a red, and time from ready to land.
- `merge-queue stats --since T --until T [--json]`: batch-width distribution (average, min/max, counts of widths 1/2/3/4), success rate by width, landed changes per validate-hour, and ready-to-land latency percentiles.
- Land small pieces as they are ready.

# Acceptance Criteria

- Every planned validate writes exactly one record; rotation keeps the directory bounded.
- `stats` reports the listed metrics for a window, in text and JSON, with --help covering every argument.
- Tests cover a red batch (culprit recorded), a donor-validated batch (validator host recorded) and rotation.
