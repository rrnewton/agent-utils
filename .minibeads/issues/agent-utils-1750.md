---
title: 'agentctl-tests-load-sensitive: runner and chat-stop tests fail under host load on wall-clock budgets'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-16T22:50:00.000000000+00:00
updated_at: 2026-11-17T05:02:00.000000000+00:00
closed_at: 2026-11-17T05:02:00.000000000+00:00
---

# Description

Gate runs for #1732 (2026-11-16) went red twice on tests no change in
the stack touches, each time only under host load:

- `py/agentctl/foreign/tests/test_agent_runner.py`:
  `test_packaged_runner_resumes_two_durable_turns[False/True]` ("runner
  failed to consume turns, return code None" after a 10 s deadline),
  `test_stop_waits_for_harness_identity_publication` and
  `test_crashed_runner_owned_harness_is_cleaned_before_state_reuse[restart]`
  (missing state files after 5-8 s waits). The same tests pass in about
  8 s unloaded, on the gated tree and on untouched `github/main` alike.
- `py/tests/test_agentctl_chat_launch_rust.py`:
  `test_run_sigterm_during_startup_exits_within_five_seconds[at-exec]`:
  the bridge's stop, slowed by load, crossed the product's real 4 s
  termination-guard grace (`CHAT_STOP_GRACE`) and logged "did not stop
  within", which the test asserts never appears. The 4 s budget is real
  product behaviour (#870); the test's verdict was the clock's.

Owner rule 346: real-time-sensitive tests wait on conditions, not
wall-clock budgets. These fail under load, so the gate cannot
distinguish a regression from a busy host.

# Design

- Foreign runner tests: every wait polls its observable condition (a
  state file exists, a turn is consumed, a process is gone) with a
  generous bound that only reports a hang; no fixed 3-10 s deadline is
  a verdict, and no sleep is a synchronisation point whose expiry
  fails the test.
- Chat stop: separate the logic from the clock. The product gains a
  test-only stop-grace override (`AGENTCTL_CHAT_STOP_GRACE_S`, seconds;
  default unchanged). (a) The clean-stop test sets a generous grace
  and asserts the clean route by its event (`chat run stopped`, and no
  abandonment line), condition-based. (b) A new abandonment test sets
  a tiny grace with a deliberately blocked step (the stalled-plugin
  handshake) and asserts the "did not stop within" line and the exit,
  condition-based. Neither test's verdict is an elapsed time.
- Proof: the touched tests pass 5 times in a row under deliberate CPU
  load (a busy loop on most cores, started and killed by literal PID
  for the proof only), and failed under the same load before the fix.

# Acceptance Criteria

- The four foreign-runner tests and both chat-stop tests pass 5x under
  deliberate CPU load, and pass unloaded.
- No assertion in the touched tests is an elapsed-time budget; bounds
  remain only as hang reports.
- `CHAT_STOP_GRACE` default behaviour is unchanged (4 s).
- `make validate` passes on the gated HEAD.

# Notes

Related: #870 agentctl-service-slow-stop, #1732
agentctl-muse-midturn-typed-not-submitted (the stack this unblocks).

## Resolution

Landed 26bd34a7 on github/main (below #1732's c3ff6fac). The foreign runner tests wait on their state-file and process conditions with generous hang-report bounds, and the chat stop is split from the clock: test-only `AGENTCTL_CHAT_STOP_GRACE_S` (default still the product's 4 s) lets the clean-stop test assert its route by log event and a new blocked-driver test assert the abandonment line by event. Proof: the old packaged-runner test failed in 1 round at 2 cores with 12 CPU spinners; after the fix all 9 test instances passed 5/5 rounds under the same load, unloaded, and in the gated validate.
