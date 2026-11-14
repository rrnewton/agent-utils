---
title: 'agentctl-lib-tests-load-robust: agentctl lib tests fail under load at default parallelism'
status: open
priority: 1
issue_type: task
labels:
- agentctl
- tests
- flake
created_at: 2026-11-14T00:00:00.000000000+00:00
updated_at: 2026-11-14T00:00:00.000000000+00:00
---

# Description

[muse ultra] `cargo test --manifest-path rs/agentctl/Cargo.toml --lib` at default parallelism on a hot host (>75% CPU on 316 hardware threads) fails 13-16 tests per run, with the set varying between runs; every failing test passes alone. Owner rule 346: gating tests must be fast, high-signal and robust on a hot machine, and nothing real-time-sensitive may gate. The failing set is mostly `chat_service` owner-loop ordering tests (pane reads versus typing), plus `client::process_liveness` / `real_non_shell_process`, subagents lock_rank / restore / transient recovery, `team_driver::driver_ack`, and agent bounded_drain / cancellation tests — 19 unique tests across two observed runs.

# Design

Replace wall-clock waits and ordering assumptions with event or condition waits and generous timeouts (a timeout is a hang guard, never the verdict). Fix tests, not product; a real product race found along the way is fixed with its own test and called out.

Root causes: every test in the process shares one target-lock root keyed by pane digest, and the fixtures reuse one pane, so fake-clock lock polling spins behind other tests' drains; the fake Herdr event server panicked when a starved handshake missed the client's acknowledgement deadline, killing later subscriptions; the owner-loop harness treated its limit as a CPU/wall verdict instead of a hang guard; several tests staged state changes from a poller thread after fixed 300/500 ms windows that can contain no scan at all on a loaded host; one test measured a 5 s bound with the test process's own clock; process tests pinned a child's identity before its exec landed and let a 30 s child outlive its fixture wait.

# Acceptance Criteria

- The full `cargo test --manifest-path rs/agentctl/Cargo.toml --lib` at default parallelism on a hot host passes 3 runs in a row.
- No test is disabled to get green; every formerly flaky test still asserts its original behaviour, now on a condition.
- Any real product race found is fixed with its own regression test and called out in the commit body.

# Notes

Number assigned from the originating host's block; filed in the delivering commit. Companion: #1551 agentctl-chat-launch-wallclock-tests (the Python edition of the same rule).

Fixes are in the lane's commits; acceptance criterion (3 full hot-host runs at default parallelism) runs in the booked coordination window, after which this file is closed with its Resolution in the delivering commit.
