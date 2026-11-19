---
title: 'wrkslots-shard-cpu-budgets-hot-host: per-step CPU budgets for the wrkslots shards are too tight on a hot host'
status: open
priority: 0
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-19T00:00:00+00:00
updated_at: 2026-11-19T00:00:00+00:00
---

# Description

[watermelon-full-preview] Owner rule 346 evidence gathered while gating #1824
`wrkslots-submodule-remote-refusal-names-reason` (2026-11-19): the
non-lifecycle wrkslots Python shards `python.wrkslots.shard-4` and
`python.wrkslots.shard-7` repeatedly hit their per-step CPU budget
(`CPU-TIMEOUT >2400s cpu`) with zero failing tests, on a loaded host
(load 150-290 that day).

Measured with `pytest --durations=25` under the host heavy slot at
`nice -n 10`, same shard spec run sequentially:

- unmodified github/main: shard 7 consumed 2190 s of CPU (user+sys)
  against the 2400 s cap — within 9% of the cap with every test passing;
- the gating tree: 2777 s under load, again with every one of the 354
  selected tests passing when run outside the DAG cap.

The totals are dominated by a few timing and watchdog tests at
100-600 s each (`test_reap_stray_worktrees.py`,
`test_worktrees_image.py`, `test_remove_write_rust.py` families), whose
durations expand under host contention, so a shard that fits on a quiet
host overruns on a hot one.

# Design

- Give the wrkslots shard steps CPU budgets with headroom for a hot
  host (raise the cap, split the heavy timing families into their own
  step, or budget from measured p95 durations rather than quiet-host
  medians). Any of these satisfies this issue; the current 2400 s cap
  against a 2190 s quiet-main baseline does not.

# Acceptance Criteria

- `python.wrkslots.shard-4` and `python.wrkslots.shard-7` pass inside
  the gate on a loaded host without CPU-TIMEOUT, with the same test
  selection as today.
- The budget change is justified in the change by measured durations
  (the numbers above or a fresh measurement).

# Notes

Filed from #1824's gating evidence with a coordinator waiver for those
two steps on that tree; the waiver does not close this issue.
