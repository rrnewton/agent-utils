---
title: 'merge-queue-donor-rebuild-wait-timeout: cli_donor test a_mid_run_root_donor_job_records_the_base_it_was_planned_on gives the stale root 60 s of wall-clock time to rebuild on the new base, and missed it at load1 about 200'
status: open
priority: 1
issue_type: bug
labels:
- merge-queue
- validation
- tests
created_at: 2026-11-27T08:10:00-05:00
updated_at: 2026-11-27T08:10:00-05:00
---

# Description

`rust.merge-queue.test` failed in the #2318 gate on 1b0c8419 (ticket `1795777600642083805-50cbd6b7`, 2026-11-27). One case of 686 failed. Nothing in #2318 touches merge-queue.

```
cli_donor/cli_donor::a_mid_run_root_donor_job_records_the_base_it_was_planned_on:
thread 'a_mid_run_root_donor_job_records_the_base_it_was_planned_on' panicked at merge-queue/tests/cli_donor.rs:163:5:
timed out waiting for change 2 to rebuild on the new base
case-timings run-cargo: 686 case(s), 683 passed, 1 failed across 57 test binar(ies)
```

The case ran from 7:44:06 to 7:45:10 AM ET (step journal). Host load (load1) was 190 at 7:43, 203 at 7:44 and 196 at 7:45, out of 316 hardware threads. That's below the 237 that counts as a hot host.

The test (`rs/merge-queue/tests/cli_donor.rs`) bounds its steps with wall-clock time:

- Its three `wait_for` calls each allow `Duration::from_secs(60)`:
  - change 1 lands on a donor receipt;
  - change 2 rebuilds on the new base, which this run missed;
  - the mid-run root job is published.

  `wait_for` panics when the time is up.
- The validator command it installs also has a clock. Change 2's first validation waits for a gate file the test never writes, polling `1200 x sleep 0.05`, so it exits 0 on its own after about 60 s.
  - The rebuild of change 2 is counted by a third marker line.
  - If that rebuild has to wait for the stale in-flight job to end, the test races two clocks of the same length. Its 60 s starts when change 1 lands; the stale validation's 60 s started earlier.
  - That dependency isn't confirmed. It should be checked first.

Owner rule 346 forbids wall-clock budgets in gating checks: a gating check waits on an event or condition with a generous bound, and passes on a host above 75% CPU.

# Design

- Replace the 60 s `wait_for` bounds in this test with a generous bound that only reports a hang, for example 600 s. Change the shared `wait_for` default in `cli_donor.rs` if other cases use the same pattern.
- Remove the validator script's own 60 s timer. Either the test writes change 2's gate file when it wants that validation to end, or the script waits for the file with a bound far above any wait in the test. That way the stale job never ends on a timer the test depends on.
- If the rebuild does wait for the stale job, end that job explicitly in the test, by writing its gate file after change 1 lands, instead of letting it time out.
- Look over the other `cli_donor` cases for the same 60 s pattern, and fix them in the same change.

# Acceptance Criteria

- `a_mid_run_root_donor_job_records_the_base_it_was_planned_on` has no 60 s wall-clock budget. Its waits are conditions with a generous bound, and its validator script ends on a condition the test controls.
- The case passes with the host's load1 above 237, for example 20 consecutive runs while a full gate runs.
- The whole `rust.merge-queue.test` step stays green.

# Notes

- Related: owner rule 346 (no real-time-sensitive gating checks); #1750 agentctl-tests-load-sensitive took the same approach in agentctl.
- Found by cl-shardcap in the #2318 gate. Step log: `rust.merge-queue.test.log` in gate log directory `1795777600642083805-50cbd6b7`.
- The exclusive rerun of both red steps on the same tree passed (ticket `1795784636031325182-279d838b`, 8:10 to 8:21 AM ET): this case passed, and `rust.merge-queue.test` had 0 failed of 686. load1 during the rerun was 107 to 154, lower than during the failure, so the pass shows only that the timeout is intermittent, not that the case is robust under load.
