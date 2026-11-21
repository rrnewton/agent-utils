---
title: 'agentctl-lib-load-flakes: two agentctl lib tests fail only under full-suite load'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- robustness
created_at: 2026-11-21T03:02:08-05:00
updated_at: 2026-11-21T03:02:08-05:00
---

# Description

[watermelon-full-preview] Two Rust `agentctl --lib` tests fail only when
the whole library suite runs in parallel, and pass on their own:

- `headless::wrapper::tests::interrupt_with_a_message_makes_it_the_next_turn`
  (panics "later echoed" at `rs/agentctl/src/headless/wrapper/tests.rs`)
- `subagents::tests::muse_followup::a_watchers_drain_types_a_queued_prompt_into_a_busy_muse`

Evidence from #2027 (2026-11-21, on this host): a full
`cargo test -p agentctl --lib` run on `aa84286b` reported 2859 passed /
4 failed, and a rerun on `971bdaff` reported 2861 passed / 2 failed; the
two tests above failed in both runs (the other two failures in the first
run did not repeat). Run isolated with `--test-threads=1`, both pass
(2 passed in 8.3s) on the same tree that failed them under load. The
#2027 change touches only process-ownership pinning, which neither test
exercises; this is recorded as load flakiness, not as a #2027 regression,
and was deliberately not fixed in that branch.

# Design

- Reproduce under full-suite load and find what each test waits on: a
  timed window expiring under CPU contention, or a condition the test
  only samples once.
- Convert the failing waits to condition-based waits on the state the
  test actually needs (owner rule: condition-based only); do not widen
  timeouts as the fix.
- Show the pair green in several consecutive full `--lib` runs on a
  loaded host, not only in isolation.

# Acceptance Criteria

- Both named tests pass in repeated full `cargo test -p agentctl --lib`
  runs under host load.
- The fix names the exact wait that raced, per test, in the resolution.
- No assertion is weakened: each test still proves the behaviour its
  name states.

# Notes

[watermelon-full-preview] Filed by lane mu-pidfdenoent at the
coordinator's direction during #2027 agentctl-pidfd-open-enoent-thread;
number 2031 assigned by the coordinator. No lane, host or person is
named here.
