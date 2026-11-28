---
title: 'lock-wait-test-late-waiter-race: the blocking lock-wait test probes the lock right after the holder lets go, while its own timed-out waiter can still be taking and dropping it'
status: open
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- wrkslots
- tests
- hot-host
created_at: 2026-11-28T13:08:00-05:00
updated_at: 2026-11-28T13:08:00-05:00
---

# Description

`py/wrkslots/tests/test_blocking_lock_wait.py::test_a_waiter_past_its_deadline_never_acts_and_leaves_the_lock_free` failed in gate 1795883841170273936-02c92bc8 (`python.wrkslots.shard-0`, 2026-11-28) with `BlockingIOError: [Errno 11]` on its final non-blocking probe. It passed in that gate's solo rerun and in the 199 other kept shard logs that ran the file.

The test:
1. holds the project-root flock in a helper thread;
2. calls `_locked_config(..., 0.2)`, which raises `_LockBusy` after its 0.2 s deadline;
3. releases the holder (the context manager joins the holder thread);
4. probes the lock at once with `flock(LOCK_EX | LOCK_NB)`, expecting it free.

`_locked_config` waits in the kernel through `_flock_blocking`. A daemon helper thread (`wait_in_kernel`) blocks in `flock` on a duplicated descriptor. When the caller's deadline passes, the wait is cancelled; but the helper stays blocked, and when the holder lets go it takes the lock and drops it at once (by design: a late acquisition never acts).

That take-and-drop is asynchronous to the holder's join. On a busy host the test's probe can land inside it and find the lock held. The product behaves as documented; the test checks too early.

# Design

- Test-only. Before the probe, wait for the cancelled kernel wait to finish. Join the waiter threads that `_locked_config` started: the `wait_in_kernel` threads present after the call and not before it. Use a generous hang backstop, not a clock assertion.
- Then probe: the lock must be free, and stay free.
- No product change; no wall-clock budget (owner rule 346).

# Acceptance Criteria

- The test joins its own timed-out waiter before the probe and passes on a busy host.
- `python.wrkslots` shards stay green in a gate.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28; number pre-assigned), from the #2465 gate. Log read: that gate's kept step log `python.wrkslots.shard-0.log`.

Related:
- #1808: the kernel-blocking lock wait (`_flock_blocking`).
