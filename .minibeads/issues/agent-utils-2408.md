---
title: 'profile-capture-wprof-wallclock: test_wprof_early_exit_after_ready_fails_via_pinned_identity_without_killing_workload races a 0.01 s fake-profiler exit against a 0.02 s window'
status: open
priority: 2
issue_type: bug
labels:
- dagrun
- tests
- validation
created_at: 2026-11-27T16:25:00-05:00
updated_at: 2026-11-27T16:25:00-05:00
---

# Description

`py/tests/test_profile_capture.py::test_wprof_early_exit_after_ready_fails_via_pinned_identity_without_killing_workload` checks that a wprof which exits after it announced readiness fails the capture without killing the workload.

How the test runs:

- The fake wprof (`_fake_wprof(..., early_exit_after_ready_s=0.01)`) announces readiness, sleeps 0.01 s and exits 0.
- The trial callback sleeps 0.03 s, reports the guest launch, then sleeps 0.12 s.
- The capture uses `wprof_window_s=0.02`.

The test asserts that the trial's error contains both "wprof exited" and "wprof produced incomplete artifacts".

`_WprofStopController._run` (py/dagrun/profile_capture.py) reports the exit only if it sees the profiler gone in one of two places:

- before the guest launch: "wprof exited after readiness but before guest launch";
- or before the window's deadline: "wprof exited before the selected window ended".

The fake exits 0, so the later `wprof exited {returncode}` clause never applies. Whether "wprof exited" appears therefore depends on whether the fake's 0.01 s sleep and exit finish before launch time plus 0.02 s. On a loaded host they often don't. The controller then reaches its deadline normally, and the error is only "wprof produced incomplete artifacts".

Seen on the #2388 gate on 668e988f (step python.dagrun.shard-1, 1795799002114893727-6977b0d9):

```
tests/test_profile_capture.py:626: assert "wprof exited" in trial.error
AssertionError: assert 'wprof exited' in 'wprof produced incomplete artifacts'
```

This is an owner-346 timing-sensitive test (reply 346, 2026-11-10): it passes only when the scheduler runs a 10 ms sleep on time.

# Design

- Make the order of events deterministic instead of timed. The test's trial callback should report the guest launch only after the fake wprof has actually exited. For example: the fake writes a marker file just before it exits, and the callback waits on that marker and on the process being gone (generous deadline, event or poll, no fixed sleep). Then the controller always takes the "exited after readiness but before guest launch" path.
- Alternatively, have the fake exit on an event the test controls (a pipe it closes) before the launch.
- Either way, drop the 0.01/0.03/0.12 s sleeps from the test. The window may stay small, because it no longer decides the outcome.
- Check the file's other wprof cases (`ready_delay_s`, `early_exit_after_ready_s`, short windows) for the same pattern, and fix those the same way.

# Acceptance Criteria

- The test has no fixed sleeps deciding its outcome. The early exit happens before the guest launch by construction, and the test still asserts the "wprof exited" and "incomplete artifacts" errors and that the workload was not killed.
- It passes 10 times in a row with the host above 75% CPU (record the load1 in the closing notes).
- python.dagrun.shard-1 passes in a gate.

# Notes

- Found by cl-shardcap while triaging the #2388 gate row. coordb asked for this follow-up.
- Related: #2065 hot-host test inventory.
- Frequent-red candidates seen on the same row, for cl-tmpfix's audit annex:
  - agentctl `queue_retire::tests::dry_run_lists_without_moving_and_all_pending_takes_every_lane` (queue_retire.rs:221);
  - wrkviz `summarize_core::tests::claude_backend_receives_each_callers_own_schema` (ETXTBSY starting its fake backend).
