---
title: 'agentctl-test-flakes: three agentctl unit tests fail under a loaded gate'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- validation
created_at: 2026-11-03T02:00:20.654082844+00:00
updated_at: 2026-11-03T02:00:20.654082844+00:00
---

# Description

[opus 5.5] Three agentctl Rust unit tests fail intermittently under a loaded gate and pass when run alone. While gating #791 chat-health-wrong-registry, four runs of `rust.agentctl.test` on one commit each failed 1-2 of 1955 cases, a different case each time, none in code #791 touches. Each failure costs a lane a full rerun. (A fourth flake, the reply ID with a leading dash, is fixed by #799 chat-reply-id-leading-dash.)

1. `client::tests::recorded_pane_shell_identity_refuses_every_generation_change` (client.rs:4463): `AdapterError { message: "cannot parse process stat for pid N", kind: Unavailable }` from an unwrap. The test reads the stat of a real process that can exit or be mid-exec between spawn and read.
2. `chat_service::tests::a_reminder_the_queue_does_not_type_waits_until_the_coordinator_is_busy_and_idle_again` (chat_service.rs:19349): expected one reminder typed, got 0. The test steps through wall-clock phases (3 s hold, 200 ms working, then idle), and under load the reminder misses its window.
3. `subagents::tests::revive::revive_refuses_orphaned_setup_with_runtime_claims_or_unrecognized_artifacts` (revive.rs:1090): the registry snapshot after the refused revive has `.waiting` and `.waiting/worker` directories the snapshot before lacks, so "a refused revive leaves the registry unchanged" fails.

# Design

- (1) Hold the process alive (or tolerate a vanished process as "generation changed") instead of unwrapping a racy stat read.
- (2) Drive the phases from the fake clock or wait on the observable state change, not wall-clock sleeps.
- (3) Find what creates `REGISTRY/.waiting/worker` during a refused revive. If the revive registers a waiter before refusing, that is a product bug (a refusal should leave nothing behind); if it is a background thread from an earlier step, isolate the test's registry.

# Acceptance Criteria

- Each of the three tests passes 200 consecutive runs while the full agentctl suite runs alongside it on a loaded host.
- No test is marked ignored or given a longer sleep as the fix.
