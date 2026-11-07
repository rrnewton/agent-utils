---
title: 'merge-queue-flaky-probe-tests: two donor/probe tests time out under host load'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-07T15:02:09.000000000+00:00
updated_at: 2026-11-07T15:02:09.000000000+00:00
---

# Description

Two merge-queue tests flake under host load, so every lane's gate pays
a solo rerun and a consuming project's suite cannot get two clean
full runs:

- `cli_confirm_async::two_reds_probe_side_by_side` (added with the
  probe window flights) times out waiting for four donor jobs
  (`timed out waiting for four donor jobs`, about 241 s in the failing
  run). It passed one full-suite run, failed the next, and passes
  about two of three solo runs: a race or a timing dependence, in the
  test fixture/donor timing or in the probe ordering itself.
- `cli_push_retry::a_head_stop_logs_at_once_and_detaches_the_in_flight_validation`
  parks behind a slow validation and fails its timing expectation
  under load (the run parked behind the slow validation for about
  121 s).

# Design

- Find why each test depends on wall-clock speed: a fixture lifetime,
  a fixed sleep, or a deadline that scales with nothing. Fix the
  cause the way the earlier differential flake was fixed, not by
  raising timeouts blindly.
- If the cause is a real ordering bug in the probe window flights,
  fix the ordering; coordinate through the coordinator if the cause
  sits in code another lane is actively fixing.
- Prove the fix with a loop under load (30 runs alongside a CPU hog)
  before and after.

# Acceptance Criteria

- Both tests pass 30 consecutive runs under CPU load.
- Both tests still assert their original behaviour (probe overlap;
  head-stop detachment), not a weakened version of it.
- The whole merge-queue crate suite is green on the landing tree.

# Notes

Reported 2026-11-07 from a consuming project's full-suite runs;
consumers are referred to generically here, per repository
convention.
