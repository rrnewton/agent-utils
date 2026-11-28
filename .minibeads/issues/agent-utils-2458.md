---
title: 'merge-queue-cpu-budget-2458: rust.merge-queue.test outgrows its 2400 CPU-s budget (red in 12 gates on 7 heads since 11-27)'
status: open
priority: 1
issue_type: bug
assignee: mu-mqcpu
labels:
- merge-queue
- validation
- hot-host
created_at: 2026-11-28T07:35:00-05:00
updated_at: 2026-11-28T07:35:00-05:00
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
