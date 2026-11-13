---
title: 'merge-queue-fake-dagrun-etxtbsy: write the fake dagrun by rename, never exec a file being written'
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-13T00:19:22.921311438+00:00
updated_at: 2026-11-13T00:25:53.505109847+00:00
closed_at: 2026-11-13T00:25:53.505109737+00:00
---

# Description

A merge-queue validation unit test failed under gate load with the validator could not execute: cannot start its fake-dagrun helper, Text file busy (os error 26). The test writes its fake executable and execs it at once; on a hot machine another thread's fork can still hold the write descriptor when the exec happens, and the exec fails. The failure is in the test helper, not in the queue logic (the same test passes alone), but under the robust-on-a-hot-machine rule it needs a real fix rather than a rerun.

# Design

In rs/merge-queue/src/validate.rs tests: add a write_fake_dagrun helper that writes the script under a temporary name, chmods it, then renames it into its final path, so no exec ever targets a file still being written. Use it at every fake-dagrun site (the early-red journal test, the effective-selection test, and the dagrun_ran helper). Because a sibling thread's forked child can still hold a duplicate of the write descriptor at the exec instant on a loaded host (measured: rename alone still fails intermittently), the test runs go through a run_fake helper that retries a spawn refused as Text file busy a bounded number of times.

# Acceptance Criteria

Every fake-dagrun script in validate.rs tests is installed by the rename helper (no direct write to the exec'd path). The previously failing setup-timeout case and the other dagrun-helper tests pass, including under the full merge-queue suite. cargo test -p merge-queue (all targets) passes.

# Notes

Evidence: a gated validate run relayed to this lane (one red case of 666 in rust.merge-queue.test; exec-time Text file busy under load, passes solo). Reserved by the coordinator during the 2026-11-12 drain work.
