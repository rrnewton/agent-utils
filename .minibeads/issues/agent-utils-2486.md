---
title: 'lock-rank-priority-test-timing: the agentctl priority-lock test orders its waiters with millisecond sleeps, so a plain waiter can take the lock before the priority one'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- tests
- hot-host
created_at: 2026-11-28T13:08:00-05:00
updated_at: 2026-11-28T13:08:00-05:00
---

# Description

`rs/agentctl/src/subagents/tests/lock_rank.rs::a_priority_manager_takes_a_contended_lifecycle_lock_first_and_waiters_are_visible` failed in one gate (head faa276a2, `rust.agentctl.test`, 2026-11-27).
- Expected `order.first() == "bridge"` (the priority manager first).
- Got `["agent-a", "bridge", "agent-c", "agent-b"]`.

The test paces its contending waiters with sleeps: a `Paced` runtime every 50 ms, and a 30 ms hold after each acquisition. It expects the priority waiter to win as soon as the holder lets go. On a busy host a plain waiter that is already spinning can win first. The ordering it asserts depends on scheduling, not on a condition the lock provides.

One occurrence in the kept logs: the console gate log in that lane's scratch directory.

# Design

- Make the test wait on conditions, not sleeps (owner rule 346).
  - Release the holder only once every waiter is registered and blocked (the test already waits for `waiters_reach(..., 4)`).
  - Assert the lock's guarantee as the product states it: the priority waiter takes the lock before any plain waiter that was waiting when the holder let go.
  - If the product does not guarantee that order, the assertion must match what it does guarantee.
- No wall-clock budget.

# Acceptance Criteria

- The test passes on a busy host without sleeps setting the order.
- `rust.agentctl.test` stays green in a gate.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28; number pre-assigned), from the 48-hour triage of `rust.agentctl.test` reds. Unassigned.
