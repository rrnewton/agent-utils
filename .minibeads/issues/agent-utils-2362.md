---
title: 'final-wait-adapter-timeout: the final-wait failure test gives its adapter only 0.5 s to start'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- validation
created_at: 2026-11-27T03:24:49-05:00
updated_at: 2026-11-27T03:24:49-05:00
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
