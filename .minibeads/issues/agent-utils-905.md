---
title: 'browser-latency-load-sensitivity: vibe-talk browser latency budgets still miss under host load after the CPU-budget rework'
status: closed
priority: 2
issue_type: bug
assignee: mu-validate
created_at: 2026-11-05T18:04:37.530921232+00:00
updated_at: 2026-11-06T11:40:00.000000000+00:00
closed_at: 2026-11-06T11:40:00.000000000+00:00
---

# Description

[muse watermelon-full-preview] Filed from the #827 gate-flake readout on 2026-11-05.

`vibe-talk.check.fold-latency-browser` failed 5 first-run gates in the last 24 h (ledger: 94 first-run gates, 57.4% pass) and `vibe-talk.check.scroll-stability-browser` failed 3, although both pass solo. The fold check (the Shift+O/e main-thread budget) was reworked under the owner's timeout rule in #866, yet it is failing again at host load 80-119, even after #866: a lane saw it fail 4/4 solo runs interleaved at load ~115 and 1/2 on main. The budget is still load-sensitive on main.

# Design

Pick one, with measurement, and record the choice in this issue:

- measure the fold's main-thread/CPU time (performance marks, long-task timing) against a tight CPU budget and relax the wall budget to ~8x, per the owner's #725 rule, if the current check does not already do exactly that; or
- raise the budget to a level a loaded host meets while a real regression still misses it; or
- skip with a named reason when measured host load makes the measurement meaningless, never silently.

Give the scroll-stability walk (a scrollTop-driven walk with no keys, which moved under load) the same treatment. No longer sleeps and no blanket skips: a real main-thread regression must still fail.

# Acceptance Criteria

- Under artificial host load comparable to load 80-119, both checks pass repeatedly (at least 10 consecutive runs) on unchanged code.
- A deliberately introduced main-thread regression of the size the budget exists to catch still fails both checks.
- Solo runs on an idle host behave as before.

# Notes

Named in the #827 readout of 2026-11-05. #866 latency-tests-cpu-budget is closed; this is the remaining load sensitivity it did not remove.

Filed by mu-validate on 2026-11-05 from the #827 gate-flake readout (local number 900); renumbered to 905 on landing by the coordinator, since 900 was taken on main.

# Design decision and measurement (mu-validate, 2026-11-06)

Load method (reproducible, bounded): the test and `stress --cpu 6` share one
`systemd-run --user --scope -p CPUQuota=200%` scope, so the browser's own
processes contend for two cores regardless of the host's 316; each run is
minutes, the generator dies with the scope, and nothing is pinned. Plain
`stress --cpu 96` on this host does NOT reproduce anything (the calibration
factor stays x1.0: 96 spinners do not saturate 316 cores), which is why the
gate failures at "load 80-119" need a contended slice, not a load number.

Fold (reproduced, then fixed): under that contention the fold check failed
intermittently with `e` handlers `170, 14, 15, 15 ms` (49 work-equivalent at
load x3.4) against the 21 ms budget. The 170 ms is one press descheduled by
the OS mid-handler for ~14 ms of work identical to its sibling presses; the
median passed and the per-press maximum (2x budget) failed. No calibration
factor can divide a single deschedule away. Choice, per the design's first
option kept tight rather than relaxed: the per-press ceiling is now the
second largest of the four presses at 2x the CPU budget, with the median
still biting at the budget. Sustained main-thread work lifts every press, so
a real regression still fails both statistics. After the change, 10/10
consecutive fold runs pass under the same contention (before: 1 of 3
failed), idle behaviour is unchanged, and a deliberate regression (80 ms of
busy work injected into every O/e keydown of a copied page) still fails with
handlers 92-96 ms against the 21 ms budget.

Scroll-stability: measured under the same scope. Its judgements are
positions within a pixel, not times, and since #866 they already gate on
observed stability with forgiving wall deadlines (`settled()`), so the same
treatment there is: no change. It passed 10/10 consecutive runs under the
contended scope on unchanged code (four batches: 1 + 3 + 3 + 3), and the
fold check passed 10/10 consecutive runs under the same scope after its
fix, with idle runs unchanged (handlers 11-26 ms at load x1.0, as before).

## Resolution

Landed as f605b64e: the fold check's per-press ceiling is the second-largest press at 2x the budget, and the median is unchanged. The acceptance record above was measured under bounded contention. The full gate was green.
