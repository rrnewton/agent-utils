---
title: 'gate-flake-stop-busy-runner: python.agentctl.shard-1 test_stop_busy_runner fails under load on signal timing'
status: closed
assignee: mu-flakes
priority: 1
issue_type: bug
created_at: 2026-11-06T14:30:00.000000000+00:00
updated_at: 2026-11-06T19:30:00.000000000+00:00
closed_at: 2026-11-06T19:30:00.000000000+00:00
---

# Description

python.agentctl.shard-1 failed in 5 first-run gates in about 24 h and passed each time it was rerun alone. The failing test is test_stop_busy_runner (the signal-codex variant), which times a SIGTERM against a busy fake runner.

# Design

Make the test wait on an observable event (the runner reporting busy, the signal delivered) instead of a fixed time budget, or scale its timeouts by measured load, without weakening what it asserts.

# Acceptance Criteria

- 20 consecutive runs pass under the bounded contention used for #905 (stress in a CPUQuota scope).
- The test still fails if stop does not deliver the signal.

# Notes

Filed 2026-11-06 9:30 AM ET at the lead coordinator's request, from one host's gate ledger and disk survey.

## Resolution

Fixed in the landing commit for this issue (agentctl tests only; no product
code changed, no assertion weakened):

- `test_stop_busy_runner_terminates_owned_harness_group[signal-codex]` waited
  5 s wall for SIGTERM -> watchdog -> kill-tree -> exit, while the product's
  own harness cleanup budget is 10 s (`KILL_WAIT_S`). The waits were already
  event-polled; only their outer bounds were below the cleanup budget. The
  stop-path waits now share a named 30 s bound (`_STOP_EXIT_TIMEOUT_S`:
  cleanup budget + watcher join + bookkeeping), and the post-kill death poll
  is 15 s. Sibling folded in at the coordinator's direction: `Rig.until` in
  the chat-runtime tests (58 call sites) had the same shape — an event-driven
  wait with a fixed 5 s no-progress bound — now a named 30 s bound
  (`_EVENT_WAIT_S`); its `pytest.fail("runtime produced no completion...")`
  path and message are unchanged. The five direct `events.get(timeout=5)`
  call sites outside `Rig.until` are untouched.

Contention proof (#905 method: `stress --cpu 6` inside
`systemd-run --user --scope -p CPUQuota=200%`):

- signal-codex: before 2/20 passed, after 20/20 passed (after-fix runs took
  up to 24 s each under contention, confirming the cleanup genuinely needs
  more than the old 5 s). Solo: 4/4 variants pass.
- Deliberate regression: with SIGTERM handling neutered, [signal-codex]
  still fails (30.5 s); a wedged chat runtime (`until(lambda: False)`) still
  fails with the exact "runtime produced no completion" message (30.6 s).
- Chat nonce test: failed 1/1 in-gate (330 s shard), passed alone 3/3 in
  ~0.3 s; the bounded-scope loop did not reproduce it unfixed (20/20), so
  that loop is not discriminating for it — reported plainly. After: 20/20
  under the scope, 142/142 chat-runtime tests solo.

Gate: `python3 scripts/validate.py --keep-going` on the exact tree passed
(48 steps, 0 failed, 0 skipped beyond selection). One leftover fixture
process from the gate (chat bridge, exact PID) was stopped by PID.
