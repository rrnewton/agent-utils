---
title: 'final-wait-adapter-timeout: the final-wait failure test gives its adapter only 0.5 s to start'
status: closed
closed_at: 2026-11-27T13:34:08-05:00
priority: 2
issue_type: bug
labels:
- agentctl
- validation
created_at: 2026-11-27T03:24:49-05:00
updated_at: 2026-11-27T13:34:08-05:00
---

# Description

[opus 5.5] `py/tests/test_herdr_chat_transport.py::test_command_final_wait_failure_always_wakes_anchor_then_reaps_supervisor` runs `_run_command(command, timeout=0.5)` and counts on that timeout to fire after the adapter is up. The test gates: it runs in the agentctl Python shards with no quarantine, skip or xfail.

On a loaded host, 0.5 s can expire too early in two ways:

- Before the supervisor publishes its anchor. The command deadline also bounds the anchor publication and the acknowledgement. The command then times out before the anchor's process descriptor is opened, the injected final-wait failure never fires, and `assert failed and woke_after_failure` fails.
- Before the adapter writes its pid file. The final `int(pid_path.read_text())` then raises `FileNotFoundError`.

This is the shape #389 fixed in the timeout test by raising its timeout. Found while fixing #2359.

# Design

Make the timeout fire on a condition, not after a fixed wall-clock interval. Change no product code.

- The adapter publishes its pid atomically (#2359), then prints one line, which wakes the command's select loop.
- The test replaces only the chat module's view of the clock: real time, plus a jump past every deadline once the pid file exists. Before publication, deadlines run on real time. After it, the next deadline check raises `TimeoutExpired` at once. Deadlines computed after the jump, such as the cleanup waits, count normally from there.
- The real timeout becomes a generous backstop (30 s), which only matters if the adapter never starts.

Do not use a longer fixed sleep, and do not skip the test.

# Acceptance Criteria

- The test's timeout path runs only after the adapter has published its pid. The injected final-wait failure and the anchor wake-up are still asserted.
- The test passes repeatedly with the chat module's process start slowed or under artificial load, with load1 recorded per run, and takes no longer than before when it passes.
- No product code changes.

# Notes

- Related: #389 (the same fix shape in the timeout test), #2359 (atomic pid writers in the same file).

## Resolution

[opus 5.5] Fixed in `py/tests/test_herdr_chat_transport.py` (landed as 418bfe6f, right after the #2359 fix d01382e5). No product code changed.

- The adapter publishes its pid atomically (#2359), then prints one line, which wakes the command's select loop.
- The test replaces only the chat module's `time` with `_ClockPastDeadlinesOnPublication`: real time, plus a jump past every deadline once the pid file exists. The command's timeout therefore fires only after the adapter has published its pid, and the cleanup waits taken after the jump count normally.
- The real timeout is now a 30 s backstop (was 0.5 s). It only matters if the adapter never starts. The injected final-wait failure and the anchor wake-up are still asserted.

Evidence:

- Slowed adapter start (1 s sleep before it writes its pid): the old test failed 3 of 3 runs with `FileNotFoundError` at load1 95-97. The new test passed 3 of 3, in about 1.2 s each.
- Slowed anchor publication (1 s delay in `_read_command_publication`): the old test failed 3 of 3 runs on `assert failed and woke_after_failure` at load1 97-99. The new test passed 3 of 3, in about 1.2 s each.
- Unslowed: the new test passed 10 runs in a row at load1 112.7-113.7, in 0.19-0.20 s each. The old test could not pass in under 0.5 s, its timeout.
- The whole test file passes (87 cases, load1 112.7), mypy is clean, and the full gate on 3af56dc3 passed 61 of 61 steps.
- Not done: runs above 75% host CPU. The host was not hot, and the slowed-start and slowed-publication checks show both failure modes are gone.
