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
updated_at: 2026-11-28T01:38:00-05:00
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
- Quarantine under owner rule 346 (liaison, 2026-11-28, as a condition of the step waiver for the #2383 gate): until this issue is fixed, mark the case so that gates skip it, with the same form the #1765 and #2065 quarantines use:

  ```rust
  #[ignore = "#2378 merge-queue-donor-rebuild-wait-timeout (owner rule 346): a 60 s wall-clock wait for the stale root's rebuild fails about 3 runs in 5 above load1 270; run on demand with cargo test -- --ignored"]
  ```

  The first code change to merge-queue adds this line, or this issue's fix replaces it. Either way, the case comes back only when it meets the acceptance criteria above.
- Seen again in the #2383 gate on f267fac3 (ticket `1795840912275062057-23be4040`, 2026-11-28): the same panic at `cli_donor.rs:163`. The case ran 64 s, 12:44 to 12:45 AM ET, with load1 at 377, then 298, then 243. The liaison waived the step for that landing instead of a rerun.
- Reproduced alone: the failing case only, 5 runs per tree, with the gate's release flags, under a private temp root.

  | Tree | Passed | Failures | load1 |
  |---|---|---|---|
  | #2383 tree | 2 of 5 | two timeouts at `:163`; one panic at `:174` (see the last bullet) | 310 to 333 |
  | main's code (aace00b3) | 2 of 5 | three timeouts at `:163` | 271 to 333 |

  About 3 runs in 5 fail once load1 is above 270, whether or not #2383 is present.
- A second fault for the same change: the test's `KillOnDrop` covers only the donor daemon.
  - On a panic, the `merge-queue run` child from `spawn_run` is dropped without a kill, so it keeps running.
  - In one main-code run it recreated an empty fixture directory (`queue/.git/merge-queue`) under the run's temp root after the temp-root wrapper had removed it.
  - Fix: wrap the `run` child in `KillOnDrop` too, and check that no process from the case is left after it.
- A third failure shape: `item_status` panicked at `:174` on `Option::unwrap()` of `None`, 7 s in.
  - The panic came while the test waited for change 1 to land: `list --all --json` returned no entry for change 1 at that moment.
  - Either the helper should treat a missing entry as "not yet" and keep waiting, or `list` read the queue state while it was being rewritten. The second would be a product bug, so check which one it is first.
