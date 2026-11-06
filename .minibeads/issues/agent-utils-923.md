---
title: 'agentctl-tmux-test-server-leak: tmux tests leave private tmux servers running inside the slot'
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-06T03:35:00.000000000+00:00
updated_at: 2026-11-06T12:20:00.000000000+00:00
closed_at: 2026-11-06T12:20:00.000000000+00:00
---

# Description

agentctl's tmux tests start private servers (`tmux -L agentctl-tmux-test-<pid>`) whose working directory is inside the lane's slot, and do not always stop them. Two such servers were still running about 8 hours later in one slot and blocked the slot's removal until their pids were killed by hand.

# Design

Every test that starts a private tmux server owns a guard whose drop or teardown runs `tmux -L <socket> kill-server` and removes the socket. It runs on success, on failure and on panic. Start the server with a cwd outside the slot (for example the test's temp dir) so a leak cannot pin the slot.

# Acceptance Criteria

- After the agentctl test suite runs, including a test forced to panic mid-way, no `agentctl-tmux-test-*` server is left running and no socket is left behind.
- A check in the test harness, or a test, fails if a private test server outlives its test.

# Notes

Reported 2026-11-05 10:50 PM ET by the lead coordinator after two leaked servers blocked a slot removal.

## Resolution

Landed in 17d27fc0 (844b0e00, 8bba9264, b0038d29, 3ad60590).
- Every tmux test owns its private server, started from a temp root and killed with its socket on drop.
- Fixture children die with the test process (PR_SET_PDEATHSIG).
- The service-stop tests reap their child groups on drop or panic, with a forced-panic test.
- Validation fails a Rust suite that leaves a test binary running.

The gate was green apart from one load-timing test that passed alone.
