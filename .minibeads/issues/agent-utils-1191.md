---
title: "binding-deadline-test-flake: a 0.2-second deadline in the blocked-open cancellation test fails under load"
status: closed
priority: 2
issue_type: bug
assignee: cl-census
created_at: 2026-11-10T05:10:00.000000000+00:00
updated_at: 2026-11-10T10:40:00.000000000+00:00
---

# Description

`py/wrkslots/tests/test_census_binding_deadline.py::test_one_blocked_open_is_cancelled_before_its_sleep_finishes`
calls `cli._audit_cache_root_identity(..., deadline=time.monotonic() + 0.2)` with one checkout open delayed by
3 s, then asserts that the delayed child logged exactly one open (`assert len(log.read_text().splitlines()) == 1`,
line 213). Under gate load (2026-11-09, load 90-170 on the gate host, about 5,000 processes) the deadline fired
before the child had started and written its log line, so the log was empty and the assertion failed. Seen once in
python.wrkslots.shard-7 of a #1173 gate (d9d09a07); the code under test was untouched by that change. A
sub-second wall-clock deadline in a test is a load flake by design.

# Design

Make the test prove the behaviour without depending on host speed:
- Use a deterministic clock (inject the monotonic source or the deadline check) so "the deadline expired while the
  open was blocked" is reached by construction. Or
- Wait until the child signals that it has started (its log line or a pipe), and only then start a deadline wide
  enough to stay robust (seconds, not 0.2 s), still well under the 3 s delay.

Keep the existing assertions: the refusal text, the bounded elapsed time, no leaked descriptors, children reaped.

# Acceptance Criteria

- The test passes 20 times in a row under synthetic CPU load (daytime, with the gate admission runs/ and queue/
  empty).
- It still fails if cancellation is broken (a sabotage run that skips the cancel shows the failure), recorded in
  Notes.
- No sub-second wall-clock deadline remains in this test file without a start signal or an injected clock.

# Notes

Filed for the lead coordinator (2026-11-09) from cl-census's gate 7 for #1173. For cl-census after #1173, #1174
and #1175, or whoever takes the census-load flake family (#1084/#1102/#1188).

## Resolution

Fixed by giving every sub-second deadline in `test_census_binding_deadline.py` a start signal (cl-census, opus 5.5).
`_StartGatedClock` patches `time.monotonic` with a clock that stands still at the moment it was armed until the
delayed open has logged its start (the log line the child writes before it sleeps), then runs with the real clock;
a 60 s cap keeps a binding that never starts from hanging the test. A deadline taken from it can therefore pass only
while the open is blocked, however late a loaded host schedules the child. It covers the blocked-open test, the
twelve-roots, unbound-subjects (re-armed per turn), costly-peer (re-armed for the second turn), cleanup-failure and
uncancellable-child tests. Elapsed bounds are now measured from the logged start (the log also records
`time.perf_counter()`): the allowance plus the 1 s reap allowance plus 2 s of scheduling slack, asserted below the
open's delay, which is 10 s in the two cancellation tests. The refusal text, descriptor and reaping assertions are
unchanged.

Sabotage: with the worker cleanup's `os.kill(pid, signal.SIGKILL)` replaced by `pass` (reverted), the blocked-open
test fails with "DID NOT RAISE ChildProcessError": the child is never reaped. Unloaded: 20/20 runs of the file
(13 tests each). Under load: landed only after 20/20 runs of the file under synthetic CPU load (nproc + 64 busy
loops on a 316-core host, in a user scope, by day with the gate queue empty); the counts are in the landing report.
