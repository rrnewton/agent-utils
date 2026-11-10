---
title: "wrkviz-serve-test-hang: a wrkviz serve differential test can hang a gate shard forever; bound every Python test"
status: open
priority: 1
issue_type: bug
created_at: 2026-11-10T03:55:00.000000000+00:00
updated_at: 2026-11-10T03:55:00.000000000+00:00
---

# Description

On 2026-11-09 a gate's `python.wrkviz.shard-0` hung for 87+ minutes with no test events. A test had started
`wrkviz serve --output <pytest tmp>/wrkviz-serve-archive0 --port <N>`. The pytest worker sat blocked on a pipe read
while the server kept polling. When the server was stopped by hand, the test started a NEW server, so it would
have retried forever. That one shard held a shared gate slot for 1.8 h and blocked every exclusive phase on the
host, including an owner-priority one, until the worker was stopped at ~10:44 PM ET.

The likely site is `py/tests/test_wrkviz_serve_rust.py`:
- `_serve()` does `process.stdout.readline()` for the banner with no timeout. A server that never writes a full
  banner line (buffered stdout, a port race with `_free_port()`'s probe-then-bind over a shared 29920-29950 range,
  or a stuck startup) blocks the test indefinitely.
- `test_serve_matches_python` calls `_serve()` twice (the Python edition, then the Rust edition), which explains
  "stopped one, a new one appeared".
- The teardown `process.communicate(timeout=10)` is the only bound, and it is never reached while readline blocks.

Any other test that spawns `wrkviz serve` (for example in `py/tests/test_wrkviz_userguide_rust.py`) needs the same
check.

# Design

1. Bound the server wait: read the banner with a deadline (a reader thread or selectors with a timeout, or poll
   the port until it accepts). On timeout, fail the test with the server's stderr so far. Never retry silently.
2. Kill the child on every exit path: terminate, wait with a bound, then kill. Use a fixture or context manager
   so teardown runs on failure and timeout too.
3. Avoid the port race: let the server bind port 0 and report its port, or retry only on a definite
   "address in use" failure, with a bounded attempt count.
4. A per-test timeout for every Python shard: run_pytest_shard (or pytest-timeout) applies a default per-test
   limit, with an explicit marker for the few tests that need longer. A hung test fails with a stack dump instead
   of holding a gate slot. Apply it to wrkslots and the other suites too.

# Acceptance Criteria

- A test with a fake server that never prints its banner fails within the bound, and no child process is left.
- A test stopped mid-run (timeout) leaves no `wrkviz serve` child behind.
- run_pytest_shard enforces a default per-test timeout, and a deliberately hanging test proves it (it fails with
  a traceback within the limit).
- The serve differential still passes normally, and still passes when two gates run it at once.

# Notes

Filed for the lead coordinator (2026-11-09) after it stopped the hung worker on the gate host at ~10:44 PM ET.
P1 because a single hung test can block every gate on a host for hours. For a wrkviz-aware lane.
