---
title: 'wrkslotsd-real-python-retry-test-cpu-budget: one wrkslotsd test costs 10-17% of the rust.wrkslots.test CPU budget'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
- tests
created_at: 2026-11-26T11:14:00-05:00
updated_at: 2026-11-26T11:14:00-05:00
---

# Description

`wrkslotsd::tests::completed_scoped_journal_paths_can_be_reused_by_real_python_retries` (rs/wrkslotsd/src/tests.rs) costs 115-210 CPU seconds when it runs alone. It drives real Python wrkslots end to end twice: create, recover --abort-create, create again; then create, finish, an interrupted remove, recover, and a remove retry. Each of those runs the full Python command, including its census. The step that runs it, rust.wrkslots.test, has a cpu_timeout of 1200 CPU seconds, and that budget also covers the release test compile, which is cold on a fresh gate tree. So this one test is 10-17% of the step's budget, and under host load it is the test that tips the step into CPU-TIMEOUT.

Measured 2026-11-26: the release lib test binary was built with the step's flags and the test run alone (`--exact`, `--test-threads 1`, nice 10), alternating between an unrelated feature tree and its base:

| run | tree | load1 at start | wall | CPU (user + sys) |
|---|---|---|---|---|
| 1 | base | 211 | 371 s | 210 s |
| 1 | feature | 179 | 140 s | 137 s |
| 2 | base | 133 | 126 s | 123 s |
| 2 | feature | 128 | 117 s | 115 s |

The cost is the same on both trees. It is the test's own cost, not a regression. A full owner gate on the feature tree went red on rust.wrkslots.test with CPU-TIMEOUT: 222 of 223 tests had completed, no assertion had failed, and this test was in flight at 227 s. An exclusive rerun of the step on the same tree passed.

# Design

Keep what the test proves (a completed scoped journal path can be reused by a real Python retry, and the Rust replay then sees exactly one pending journal with that path), at a fraction of the CPU cost. In order of preference:

1. Slim it: build the retry fixture's state with the Python functions directly, in one Python process, instead of full CLI subcommands that each rerun fleet validation and the census. Or share one prepared project between the create and finish halves.
2. Split it: the create and finish halves become two tests, so neither alone is a large share of the budget and the step's case timings name the slow half.
3. Only if 1 and 2 cannot bring it down: give rust.wrkslots.test measured headroom. Raise cpu_timeout from a recorded CPU-second profile of the whole step (cold compile plus tests), not from a guess.

Judge any fix by counting, not timing (owner rule: no wall-clock budgets in gating checks). Count the Python CLI invocations and census passes the test makes, and the step's CPU seconds from its cgroup accounting, before and after. No wall-clock assertion goes into the test.

# Acceptance Criteria

- The test (or its split parts) uses at most half its current CPU seconds alone, measured as CPU seconds (user + sys) at a recorded load1, and keeps the same assertions on the replayed pending journal.
- Or, if it cannot be slimmed, rust.wrkslots.test's cpu_timeout is set from a recorded profile of the whole step with at least 25% headroom over its measured CPU seconds with a cold compile.
- The fix is verified by counted invocations or CPU accounting, never by a wall-clock threshold.

# Notes

Found during #2179 (wrkslots-transfer-owner-stale-task-scope), whose change does not reach this test: it calls no transfer-owner and records no task scope. Related: #2299 finish-archive-remove-budget (same host-load class, different mechanism), #2077 (CPU-TIMEOUT load family).
