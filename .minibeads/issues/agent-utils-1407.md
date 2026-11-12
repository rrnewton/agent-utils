---
title: 'proccpu-pidns-cache: the proccpu session cache is dropped wholesale in a PID namespace, and its test fails there'
status: open
priority: 2
issue_type: bug
labels:
- testrun
- dagrun
created_at: 2026-11-12T00:33:35+00:00
updated_at: 2026-11-12T00:33:35+00:00
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
