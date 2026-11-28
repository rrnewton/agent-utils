---
title: 'merge-queue-test-sys-share-2479: 79% of rust.merge-queue.test CPU is sys time; measure process spawns, fs syncs and git subprocess churn per test before any sharding'
status: open
priority: 2
issue_type: task
labels:
- merge-queue
- validation
- hot-host
created_at: 2026-11-28T13:05:00-05:00
updated_at: 2026-11-28T13:05:00-05:00
---

# Description

#2458 raised the CPU budget of `rust.merge-queue.test` from 2400 to 4000 CPU-s because the suite grew to 687 tests in 57 binaries. Its measurements show the real smell: sys time is 79% of the tests-alone CPU (a solo run with the build cached spent User 321.6 + Sys 1219.5 = 1541.1 CPU-s over 744 s wall), and #1051 already saw about 75%. The tests drive the merge-queue command against local bare-repository fixtures, so the kernel time is presumably process creation, git subprocesses, file-system syncs and fixture setup, but nobody has measured it. Under gate load the sys share grows: two gates on 2026-11-28 hit 2400 CPU-s with 681 and 687 of 687 tests done, against 1944 CPU-s for a solo run including compilation.

Two side observations from the same gate kills belong here too:

- After every test had passed, `cargo test --doc -- --list` was rebuilding the merge-queue library (rustc at 74.9 CPU-s when the cap hit), although the test binaries were built moments earlier with the same flags. A doctest phase that reuses the build would save that CPU on every run.
- The gate journal's `tests_started` counter reported 1121 and 1131 for a step whose log has 687 `##TEST-START` lines; that over-count is what made the liaison report read "about 1,060 tests".

# Design

- Pick one or two heavy binaries from #2458's ranking (`cli_donor`: 354 s summed wall over 44 tests; `cli_disjoint_root`: 165 s over 17) and run them alone under `nice -n 10`, with `RUST_TEST_THREADS=1` so per-test attribution is clean, under `strace -c -f` or `perf trace -s`. Record per test: processes spawned (fork, clone, execve) and by which program (git, the merge-queue binary, shells), fsync/fdatasync/sync_file_range calls, file creations and renames, and the sys CPU-s they account for. Record load1 at start and end of every run.
- Repeat one binary at a hot host (load1 >= 237 for the whole run) to see which part of sys grows with load (scheduler and page-table work in fork, lock contention, fsync latency).
- From the counts, name the top sources of sys time with their share, for example: git subprocesses per queue operation, a fixture rebuilt per test that could be shared or cloned once, fsyncs that a test fixture does not need, polling that forks a process per tick.
- Check why the doctest phase of `scripts/case_timings.py run-cargo` rebuilds the library (compare the fingerprints or the `cargo test --doc` unit's flags and features against the earlier build) and whether the step can reuse it.
- Write the findings and a proposal into this issue's Notes BEFORE any sharding proposal: reduce the churn first, or shard (for example `cli_donor`, `cli_run` and `cli_disjoint_root` into a second step) only if the measured churn is inherent to what the tests check.

# Acceptance Criteria

- Per-test syscall and process counts (spawns by program, fsync-family calls, file creations) and the sys CPU-s they account for are recorded for at least one heavy binary, alone and at a hot host, with load1 per run.
- The top sources of sys time are named with their share of the binary's sys CPU, and each comes with a reduction (or a reason it is inherent).
- The doctest-phase library rebuild is explained, and fixed or filed.
- Any sharding proposal cites these measurements; none lands before them.
- No wall-clock budget is added to any gating check (owner rule 346).

# Notes

Filed by `cl-mqcpu` with the #2458 budget raise (2026-11-28; number pre-assigned by coordb): the raise lands only together with this investigation filed. Related: #2458 (the 2400 to 4000 raise, its measurement table and heavy-test ranking), #1051 (the 1200 to 2400 raise, which first measured about 75% sys, and the kill-progress wrapper).
