---
title: 'donor-test-process-leaks: donor restart tests leave merge-queue processes running for a day'
status: open
priority: 2
issue_type: bug
assignee: mu-predict
created_at: 2026-11-09T04:14:26.000000000+00:00
updated_at: 2026-11-09T04:14:26.000000000+00:00
---

# Description

Removing a worktree slot on 2026-11-08 found six `merge-queue run --json` processes leaked by one `cli_donor` test run about a day earlier, reparented to systemd and still running from deleted binaries in `/tmp` test directories for the donor-restart fixtures (legacy-pause, escalated, mid-pause - two each) and the donor claim-timeout fixture. At least one caught SIGTERM and kept running. They blocked slot removal until stopped by verified PID. The donor restart tests evidently spawn a run that outlives the test: some paths drop or detach the child handle without killing it (a plain `Child` drop does not kill), or a restart path detaches the replacement process; explicit `run.kill()` calls exist on some paths but evidently not all exits, including panics and early returns.

# Design

- Every test that spawns a daemon or run owns it for the whole test: a kill-on-drop guard (and the child's own process group or equivalent containment) so normal return, panic and early-return paths all reap it and anything it spawned.
- Apply the guard to the donor restart and claim-timeout fixtures first, then inventory the other merge-queue integration tests that spawn `merge-queue` processes directly (as opposed to the shared fixture child type) and convert the same shape.
- Each converted test asserts at the end that no `merge-queue` process with a working directory under the test's own temporary directory survives it.

# Acceptance Criteria

- The donor-restart and claim-timeout tests leave no merge-queue process behind on success or on an injected mid-test failure path, verified by the survival assertion in each converted test.
- The closing note inventories daemon/run-spawning tests found and which were converted to the guard.
- Full `rust.merge-queue.test` passes with no leaked processes from the run (a final check on the test temp directories finds none alive).

# Notes

Found 2026-11-08 during slot removal by the lead coordinator; the leaked processes were about 35 hours old and one survived SIGTERM. Filed after #1100 landed, as directed; the SIGTERM survival half may be product behaviour worth naming separately if the guard work shows the queue ignoring termination outside tests.
