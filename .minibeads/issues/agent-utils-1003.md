---
title: 'wrkslotsd-lib-test-stack-overflow: the debug lib-test binary overflows its stack; the gate only runs release'
status: open
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-07T03:50:00.000000000+00:00
updated_at: 2026-11-07T03:50:00.000000000+00:00
---

# Description

`cargo test -p wrkslotsd --lib` (run from rs/, default debug profile) aborts with a stack overflow on main. It overflows the same way on an untouched tree, so it predates #995, whose landing exposed it on 2026-11-06.

Does the gate run that binary? Yes, but only an optimised build of it. `rust.wrkslots.test` (validation/rust.dag.yaml) runs `cargo test --release --no-fail-fast -p wrkslotsd` with RUST_TEST_THREADS=4, through `scripts/case_timings.py run-cargo`, which turns any non-zero exit into a failed step. So a crash would not be tolerated. On 2026-11-06 the step passed with "97 case(s), 97 passed, 0 failed across 3 test binar(ies)", including the lib tests. Release builds use far smaller stack frames, so the overflow never shows there. No gate step builds or runs the debug test binaries, so the gate cannot see this, and a developer's plain `cargo test` hits it first.

# Design

- Find the overflowing test and frame: run the debug lib binary with `--test-threads=1` and `RUST_BACKTRACE=1`, or under a debugger, and name the test and the recursion or large stack value (big arrays or structs held by value, deep recursion over journal events, large async futures).
- Fix the cause (box the large values, make the recursion iterative). Do not just raise `RUST_MIN_STACK` or the thread stack size, unless the measurements show the depth is bounded and legitimate, and then say so here.
- Make the gate catch it: either run the wrkslotsd lib tests in debug as well (measure the added time), or add a test that runs the deepest path on a thread with the default 2 MiB stack in both profiles.

# Acceptance Criteria

- `cargo test -p wrkslotsd --lib` passes in debug from rs/ on a clean target.
- The issue names the overflowing test and frame, and the fix.
- A gate step fails if the overflow comes back, and that is shown by temporarily reintroducing it.

# Notes

Reported 2026-11-06 by the lead coordinator (found by one of its lanes while landing #995). The gate analysis above is from validation/rust.dag.yaml and scripts/case_timings.py on main, and the step's result line in a 2026-11-06 full gate; the debug overflow itself is the reporter's reproduction.
