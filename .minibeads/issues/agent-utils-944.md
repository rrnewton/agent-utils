---
title: 'gate-flake-cross-environment-cpu-timeout: python.repository-infrastructure.cross-environment hits its CPU timeout under load'
status: closed
assignee: mu-flakes
priority: 1
issue_type: bug
created_at: 2026-11-06T14:30:00.000000000+00:00
updated_at: 2026-11-06T21:00:00.000000000+00:00
closed_at: 2026-11-06T21:00:00.000000000+00:00
---

# Description

python.repository-infrastructure.cross-environment failed in 4 first-run gates in about 24 h on its CPU timeout and passed when rerun alone.

# Design

Find which sub-check spends the CPU. Cut redundant work, split the step, or make its budget scale with measured load; keep the hermeticity checks intact.

# Acceptance Criteria

- 20 consecutive runs pass under bounded contention.
- Its assertions are unchanged.

# Notes

Filed 2026-11-06 9:30 AM ET at the lead coordinator's request, from one host's gate ledger and disk survey.

## Resolution

Split `python.repository-infrastructure.cross-environment` (214 cases,
~50 CPU-s solo against its 120 CPU-s budget — a 2.4x margin erased by
all-core gate load) into six family-preserving pytest shards
(36/36/36/36/35/35 cases), each keeping the 960 s wall and 120 CPU-s
budgets. A two-shard split was measured first and rejected: the
count-based assignment (the weights file records no durations for these
families) left 51.5 of 59.6 CPU-s in one shard. Across six shards the
heaviest carries 22.0 CPU-s solo (dagrun profile user+sys), a 5.5x
margin. The stale `est_duration_s: 3` hint is corrected. No test,
assertion, or harness semantic changed.

Contention (#905 method: `stress --cpu 6` in a `CPUQuota=200%` scope):
unsplit 19/20 (one failure with kernel CPU inflated, sys 30.3 s vs
~21 s typical; failing case name not captured by the harness summary);
six shards 20/20 dagrun iterations (~31 s each). Deliberate regression:
removing the `CARGO_BUILD_JOBS` env scrub still fails
`test_an_ambient_build_width_does_not_reach_either_engine`.
