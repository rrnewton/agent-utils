---
title: 'agentctl-fixture-poison-flake: one exiting-process stat flake fails a second client.rs test through a poisoned fixture lock'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- flake
- validation
created_at: 2026-10-24T15:05:44.537667807+00:00
updated_at: 2026-10-24T15:05:44.537667807+00:00
---

# Description

[opus 5.5] Found by #498 suite-a-la-carte on 2026-10-24, in a full-graph gate of 0e438c48 on the loaded validation host (log /tmp/cl-modular-gate-0e438c48.log on that host, lines 505-516). rust.agentctl.test failed 2 of 1350 cases. client::tests::real_supported_shell_with_background_descendant_is_not_idle panicked at rs/agentctl/src/client.rs:4832 with AdapterError "cannot parse process stat for pid N" (Unavailable). That is the root cause #495 revive-stat-parse-flake already tracks (parse_process_stat refuses an empty or truncated /proc/<pid>/stat of a process exiting at that moment), here in a unit test rather than the cross differential. The panic happened while the test held the EXECUTABLE_FIXTURE guard (client.rs:3469, held by FakeExecutable), so the lock was poisoned and client::tests::workspace_allocation_passes_literal_environment then failed at client.rs:3861 on EXECUTABLE_FIXTURE.lock().unwrap() with PoisonError, a second red test from one flake. The commit under test changed no Rust; both tests passed 3 of 3 when rerun alone, and the next gate of the same tree passed.

# Acceptance Criteria

The fixture locks in client.rs tests (EXECUTABLE_FIXTURE, SIGNAL_FIXTURE) recover from poisoning (lock().unwrap_or_else(PoisonError::into_inner)), so one panicking test fails alone; the exiting-process stat case itself is fixed under #495, and this unit test is named there as a second site.
