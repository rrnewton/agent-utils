---
title: 'proccpu-pidns-cache: the proccpu session cache is dropped wholesale in a PID namespace, and its test fails there'
status: closed
priority: 2
issue_type: bug
labels:
- testrun
- dagrun
created_at: 2026-11-12T00:33:35+00:00
updated_at: 2026-11-13T20:41:11+00:00
closed_at: 2026-11-13T20:41:11+00:00
---

# Description

[opus 5.5] `native_second_snapshot_skips_other_sessions` in rs/testrun-core/src/proccpu.rs (around line 2012) fails 5/5 in a consumer's continuous job that runs in a PID-namespaced sandbox (94 visible processes): "the second snapshot read 94 stat files of the 94 the first read". The test asserts `warm * 4 < cold`. The logic landed in 2e31e526 (2026-10-04); the Python twin py/dagrun/proccpu.py (around line 256) has the same logic. It isn't specific to that consumer: any run under `unshare --pid` or a container runtime hits it. Diagnosed by a consumer's agent, read-only.

Two causes; each alone gives exactly 94/94:

- **H1, a real defect (it fails safe, but the cache becomes useless in any container):** `reallocated()` mixes namespaces. `/proc/loadavg`'s last_pid is per PID namespace, while `/proc/stat`'s processes counter (forks) is host-wide. With forks on the host but none inside the namespace, `created > 0` while `start == end`, so the wrap branch (`pid > start || pid <= end`) is true for every PID, and `SessionCache::begin` drops the whole cache.
- **H2, the environment (correct behaviour):** the test's helper uses `process_group(0)` without `setsid` (around line 1252). In a sandbox where nothing called setsid, every process shares one session, so nothing is skippable.

# Design

1. `reallocated()` returns false when `start == end` (no PID allocated in this namespace since the last snapshot), in Rust and Python, with a comment explaining the namespace mismatch. Keep the existing `created == 0` early return.
2. Make the test environment-independent: spawn 32 helpers that call setsid, assert the warm scan reads none of them, and drop the host-dependent 1/4 ratio. Extend the `PROCCPU_SESSION_CACHE` diagnostic line so a future failure says whether it was H1 or H2.
3. Add a shared fixture for a child PID namespace, a unit case for `start == end` below the lap, and a reproduction with no sudo: `unshare --user --map-root-user --pid --fork --mount-proc`.

# Acceptance Criteria

- The test passes inside `unshare --user --map-root-user --pid --fork --mount-proc` and on a plain host.
- A unit case covers `start == end` with `created > 0`: no PID counts as reallocated.
- Rust and Python agree, and both have the comment.

# Notes

The consumer bumps its agent-utils pin once this lands. Numbered from devbig030's interim block (1400-1469).

## Resolution

[watermelon-full-preview mu-pidnscache] Fixed both causes in both editions, and made the live test prove its own preconditions instead of assuming the host's session layout.

- `rs/testrun-core/src/proccpu.rs` and `py/dagrun/proccpu.py`: `reallocated()` / `_reallocated()` now returns false when `start == end`, after the existing `created == 0` early return, with a comment explaining that `/proc/loadavg`'s last_pid is per PID namespace while `/proc/stat`'s fork counter is host-wide. A shared corpus case (`host-forks-without-a-namespace-allocation-do-not-drop-the-cache`) and a direct unit case in each edition cover `start == end` with `created > 0` below the lap.
- Test helpers now call setsid: the Rust `Native` fixture leads its own session, and each edition's `native_second_snapshot_skips_other_sessions` spawns 32 additional setsid helpers, then asserts the warm scan read none of the foreign-session helpers (tracked per PID, not by the old host-dependent `warm * 4 < cold` ratio). The `PROCCPU_SESSION_CACHE` diagnostic now reports the registered session, foreign-helper count, warm foreign reads and a `cause` of `H2-helpers-not-in-foreign-sessions`, `H1-cache-dropped-or-pid-reallocated`, or `none`, and the assertion messages name the same causes. No wall-clock assertions were added.
- A gated Python case reruns the live test under `unshare --user --map-root-user --pid --fork --mount-proc`, probing unshare first and skipping cleanly where user namespaces are unavailable.

Evidence (no sudo):

- Plain host, Python: `PROCCPU_SESSION_CACHE {"edition": "python", "cold_reads": 3949, "warm_reads": 3, "foreign_helpers": 32, "warm_foreign_reads": 0, "cause": "none"}`; full `py/tests/test_proccpu.py`: 32 passed.
- Plain host, Rust: `PROCCPU_SESSION_CACHE {"edition": "rust", "cold_reads": 3961, "warm_reads": 5, "foreign_helpers": 32, "warm_foreign_reads": 0, "cause": "none"}`; full testrun-core suite passed and `scripts/check_testrun_core_deps.py` reports the fixed 8-package set unchanged.
- Inside `unshare --user --map-root-user --pid --fork --mount-proc`, Rust test binary: `cold_reads: 35, warm_reads: 2, warm_foreign_reads: 0` (passed), and the gated Python namespace case passed.
- Gate: cpuset local validate (48 cores) and `GATE_SLOTS=3 python3 scripts/gate.py -- python3 scripts/validate.py --base github/main --keep-going` both passed; validate reports PARTIAL only for 7 delegated-live-scope cross checks skipped as UNVERIFIED, with every selected node passing.
