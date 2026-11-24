---
title: 'rust-agentctl-test-frequent-reds: classify the frequent reds as real, flaky or environment'
status: open
priority: 2
issue_type: task
labels:
- agentctl
- flaky-test
- hot-machine
created_at: 2026-11-24T16:05:00-05:00
updated_at: 2026-11-24T16:35:00-05:00
---

# Description

`rust.agentctl.test` is the highest genuine frequent-red step once wholesale
gate deaths are discounted: red on 77 distinct heads in 7 days (82 reds on 80
heads raw). The frequent-red audit could not classify it, because no failing
log survived: gate logs are pruned to the last 21 gate directories, and every
failing row's log is gone. This issue tracks the classification and whatever
hardening it points to. No product fix is in the filing lane.

# Design

- Sample failing rows across distinct heads, skipping wholesale-dead gates,
  and group by failing test name and assertion where logs exist.
- Classify each group: real lane defect, flaky (timing or load, with the
  mechanism named), or environment (host state leaking into the test).
- Keep gate logs for failing rust steps long enough that the next sample is
  not blind, or state the retention change this needs.

# Acceptance Criteria

- Every sampled group has a verdict (real, flaky with mechanism, or environment) in the Notes.
- Each flaky test is named with its mechanism, or the Notes state plainly that no mechanism could be proven from the surviving evidence.

# Notes

Investigation (mu-flakeaudit, 2026-11-24). Ledger sample since 2026-11-21,
wholesale-dead gates (more than 8 failed browser steps) skipped: 68 rows over
67 distinct heads. By day: 45 on 11-21, 3 on 11-22, 15 on 11-23, 5 on 11-24.
Ten sampled heads: 83dc6d7d77, 3c90f0d88d9, a4b0786197, ab99541193,
cfdea4ef1c, cbcee58b34, e53288c5a6, faf5be0873, 577c1148d5, e6bb458fc6 —
all ten are sole-failure rows (the gate failed nothing else), all on 11-21.

Findings:

- Grouping by failing test name and assertion: NOT POSSIBLE from surviving
  evidence. Every sampled row's step log was pruned before this audit; the
  only retained rust.agentctl.test logs (2026-11-24 gates) are passes. Any
  finer grouping would be invented, so none is claimed.
- Real lane defect: none proven. The step passes alone on current main
  (72663216): 2 of 2 solo dagrun runs passed at host load 139-165, taking
  616-963 s each. A consistently broken test does not do that.
- Flaky (load): the dominant shape. 45 of 68 rows sit on 11-21, the heaviest
  gate day in the window, and co-fail with the wrkslots shard family and
  rust.wrkslots.test (18 rows each), the known CPU-timeout family. The solo
  runs show why: the step needs 10-16 minutes of wall time even alone under
  load, so in a 225-step gate its tests are the first to hit CPU-time
  budgets while waiting for cores. Mechanism: CPU-time starvation under
  gate contention, not a wall-clock wait in the test itself. The 15
  sole-failure rows show the same sensitivity without any other step
  failing: the step is simply the canary.
- Environment: one retained neighbour log (rust.wrkslots.test, same family
  and day) failed on an installed-versus-worktree path assertion for the
  sandbox helper, which is host state leaking into a test. No agentctl log
  survives to prove the same leak for this step, so this verdict is
  "possible, unproven" for rust.agentctl.test specifically.

Recommendation: do not disable the step. First make the next sample
possible (retain failing rust step logs beyond 21 gate directories, or have
the gate record the failing test names in the ledger row), then re-sample.
If a retained failing log then shows a CPU-timeout, the fix is the #2219
pattern (CPU-time budgets), not a known-red waiver.
