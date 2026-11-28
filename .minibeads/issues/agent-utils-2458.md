---
title: 'merge-queue-cpu-budget-2458: rust.merge-queue.test outgrows its 2400 CPU-s budget (red in 12 gates on 7 heads since 11-27)'
status: closed
priority: 1
issue_type: bug
assignee: cl-mqcpu
labels:
- merge-queue
- validation
- hot-host
created_at: 2026-11-28T07:35:00-05:00
updated_at: 2026-11-28T13:40:00-05:00
closed_at: 2026-11-28T13:40:00-05:00
---

# Description

`rust.merge-queue.test` runs out of its CPU budget and is red in 12 gates on 7 heads since 2026-11-27 (liaison 1171). Step logs show 2,400 CPU-s spent at about 1,029 s wall with about 650 of 1,060 tests done, and name `cli_run::culprit_is_found_with_failed_tests_only_and_the_rest_lands` as the test running when the cap hit. The step is declared in `validation/rust.dag.yaml` (group `merge-queue`, job `test`): `cpu_timeout: 2400`, `timeout: 3600`, `RUST_TEST_THREADS=4`, `cargo test --release -p merge-queue` through `scripts/case_timings.py run-cargo`. The 2,400 cap was set by #1051 when solo runs measured 738-875 CPU-s in tests alone and up to 1,189 CPU-s including recompilation. The suite has since outgrown that basis, or a test now burns CPU (busy-wait, unbounded retries), or host load inflates sys time past the cap. Owner rule 346 applies: no wall-clock budgets, robust on a hot host.

# Design

- Find where the step CPU budget is declared (validation DAG / dagrun limits) and record it.
- Measure the suite real CPU-s on this host, alone and under gate load, recording load1 for each run (hot-load means load1 at or above 237 on 316 hardware threads).
- Find the heavy tests (per-test CPU, per-binary CPU) from the case-timings records.
- Choose one: raise the budget with a measured basis and margin, or split the suite into shards, or fix a test that burns CPU. Tell the coordinator the choice with numbers before committing code.
- Keep the wall ceiling from following a CPU raise mechanically; size it from measured wall with starvation headroom, without letting a real hang hold a gate slot.

# Acceptance Criteria

- The budget declaration site is named and the measured solo and loaded CPU-s (with load1) are recorded in the landing.
- Heavy tests are named with per-test CPU, or the report states none dominates and the growth is spread.
- The chosen fix (budget raise, shard split, or test fix) lands with its measured basis in the DAG comment, and `rust.merge-queue.test` passes in a gate without approaching the new budget.
- No wall-clock budget is added (owner rule 346).

# Notes

Filed by `mu-mqcpu` at the coordinator request (2026-11-28; number pre-assigned by coordb). Related: #1051 (the 1,200 to 2,400 raise and the kill-progress wrapper), liaison 1171 (the 12-gate red report).

## Measurements (2026-11-28)

Budget site: `validation/rust.dag.yaml`, step `rust.merge-queue.test` (group `merge-queue`, job `test`): `cpu_timeout: 2400`, `timeout: 3600`, `RUST_TEST_THREADS=4`, through `scripts/case_timings.py run-cargo`. All runs below are the step form on main at the time, 687 cases in 57 binaries.

| Run | What it covers | Tests | CPU-s | Wall | load1 |
|---|---|---|---|---|---|
| Solo 1 | compilation included | 685 pass, 0 fail, 2 ignored | User 822.0 + Sys 1121.8 = 1943.9 | 724.5 s | 183.8 at start, 267.3 at end |
| Solo 2 | compilation cached, per-case timings | 685 pass, 0 fail, 2 ignored | User 321.6 + Sys 1219.5 = 1541.1 | 744.2 s | 233.6 at start, 156.5 at end |
| Gate kill, 10:37 AM ET | full gate | 681 of 687 done | 2401.7 (step cgroup) | 803 s | not recorded by the gate |
| Gate kill, 11:58 AM ET | full gate | 687 of 687 done | 2400.7 (step cgroup) | 888 s | not recorded by the gate |

- Sys is 79% of tests-alone CPU (Solo 2): git, donor and queue subprocess churn, not user compute. #2479 measures where it goes.
- Growth since #1051: 557 to 687 `#[test]` functions (+23%), test files 50 to 55.
- Heavy tests (wall, Solo 2; summed duration 1921 s across the parallel threads): `cli_donor::kept_stale_roots_keep_only_running_roots_and_rebuild_unstarted_ones_on_the_new_base` 93.9 s (by design: an 8 s validator sleep plus 30 s donor waits; wall-heavy, not CPU-busy), then `cli_front_starve` 45.5 s, `cli_stale_green` 39.7 s, `cli_stale_cancel` 36.1 s. The top 10 tests are 17.8% of summed duration and the top 50 are 39.2%: no test dominates; the growth is spread. Heaviest binaries by summed wall: `cli_donor` 354 s over 44 tests, `cli_run` 190 s over 32, the library unit tests 170 s over 290, `cli_disjoint_root` 165 s over 17, `cli_regen` 142 s over 31.
- No busy-wait or unbounded retry: production and test polls sleep 20-50 ms, bounded. The test the liaison report named (`cli_run::culprit_is_found_with_failed_tests_only_and_the_rest_lands`) took 9.7 s and passed; it was in flight at the kill, not its cause.
- Count note: the liaison report's "about 1,060 tests" is the gate journal's `tests_started` counter (1121 and 1131 in the two kills above), which over-counts. The step's own log in each kill has exactly 687 `##TEST-START` lines and the journal 685 and 687 `test_start` events: the suite is 687 cases. The 11:58 AM ET kill came after every test had passed, while `cargo test --doc -- --list` was rebuilding the library (rustc on the merge-queue library at 74.9 CPU-s when the cap hit); the journal's "the step hung after the last one" was that rebuild. #2479 asks why the doctest phase does not reuse the build.
- The 10:37 AM ET gate also reported `cli_donor::a_mid_run_root_donor_job_records_the_base_it_was_planned_on` failed, on a head that did not touch merge-queue; its output was lost to the kill (case_timings prints failure output at the end of a binary). Watch it in the landing gate.

## Basis for 4000

The loaded step needs about 2400-2600 CPU-s (the 10:37 kill extrapolates to 2423 CPU-s for all 687 tests before the doctest phase; the 11:58 kill spent 2400 with every test done and the doctest phase unfinished). 4000 is 2.06x the worst solo run (1944) and over 1.5x the loaded estimate, the same ~2x rule #1051 used. The wall ceiling stays 3600: at the slowest measured rate (2.07 cores average, Solo 2) 4000 CPU-s needs about 1930 s wall, and the gates ran at 2.7-3.0 cores. No wall-clock budget is added (owner rule 346). Sharding the step (for example `cli_donor`, `cli_run` and `cli_disjoint_root` into a second step) is the follow-up if the suite grows another ~25%, after #2479 measures the sys share; fixing one test cannot recover the budget.

## Resolution

Landed on main 2026-11-28 as ea92f6e2 (`cpu_timeout` 2400 to 4000 on `rust.merge-queue.test` only, with the measured basis in the DAG comment; the wall timeout stays 3600, no wall-clock budget added) and 685507a8 (#2479 filed, the sys-share investigation the raise was conditioned on), gated as 97fb290d.

- Full gate (12:39 to 1:11 PM ET, 364 steps): `rust.merge-queue.test` used 2177.7 CPU-s of the new 4000 (54%) at 852.75 s wall with all 687 cases run, doctest phase included; no CPU timeout. The gate's two red steps were this one, on `cli_donor::a_mid_run_root_donor_job_records_the_base_it_was_planned_on` (the 60 s wall-clock wait tracked by #2378), and `python.agentctl.shard-0`, on the pid-file race tracked by #2482; this change touches neither test.
- Exclusive rerun of both steps on the same tree (1:13 to 1:28 PM ET): passed. `rust.merge-queue.test` 2070.3 CPU-s (52%) at 875.9 s wall, 685 passed, 0 failed, the #2378 case included; load1 242.3 at its end.
- Both measured runs stay well clear of the new budget, which meets the last acceptance criterion. #2479 stays open for the sys share, the doctest-phase rebuild and the `tests_started` over-count; sharding waits on its measurements.
