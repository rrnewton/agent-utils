---
title: 'agentctl-pidfd-open-enoent-thread: pidfd_open ENOENT on Linux 7.1 must fail closed, never read as death'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- robustness
created_at: 2026-11-21T02:14:19-05:00
updated_at: 2026-11-21T02:14:19-05:00
---

# Description

[watermelon-full-preview] On the current host kernel (Linux 7.1, after the
2026-11-20 reimage), `pidfd_open` on a live non-leader thread id fails with
`ENOENT`, not the `EINVAL` that #1938 (closed) handled on kernel 6.13. A
six-line probe confirms it: start a thread, `pidfd_open` its native id, get
`ENOENT`; the leader's id and a dead pid answer normally (`ESRCH` for the
dead pid). In `pin()` (both editions) `ENOENT` escaped the `EINVAL`
fail-closed path: Rust returned `runner_identity_unknown` and Python raised
the raw `FileNotFoundError`, so a live thread id proved nothing usable and
cleanup crashed instead of refusing. Two Rust tests were red on untouched
`github/main` on this kernel:
`foreign::ownership::tests::pin_a_thread_id_is_never_the_recorded_leader`
and `stop_process_refuses_a_live_exact_match_under_einval`. Any reader that
maps `ENOENT` alone to "no such process" would be worse: a live thread
would read as dead, the dangerous direction for cleanup and stop.

# Design

- Confirm the errno on this kernel and audit every `pidfd_open` call site
  in `rs/agentctl` and the Python `agentctl`/`wrkslots`/herdr code,
  recording what `ENOENT` and `EINVAL` each lead to today.
- `ENOENT` from `pidfd_open` is dead only when procfs also shows no such
  task in `/proc`: `/proc/<pid>` absent means gone; any task still present
  (a thread, whose status `Tgid` differs from its `Pid`, or anything else)
  is unproven and fails closed exactly like `EINVAL` in #1938. Keep the
  6.x `EINVAL` path, with a thread in procfs (`Tgid != Pid`) unproven
  there too. Both editions behave the same, with mirrored parity tests.
- Tests assert the verdict, not the errno: the two red tests pass on 7.1
  and stay correct on 6.x, plus a deterministic seam test forcing `ENOENT`
  (and `EINVAL`) through the pidfd opener so it does not depend on the
  host kernel. Condition-based waits only.

# Acceptance Criteria

- The errno on this kernel is confirmed and the call-site audit (what
  `ENOENT` and `EINVAL` lead to at each site) is reported.
- `pin()`/`stop_process()` never certify death from `ENOENT` alone in
  either edition; a live thread id is unproven and is never signalled.
- The two named Rust tests pass on Linux 7.1, their Python mirrors pass,
  and the new forced-errno seam tests pass on either kernel.
- No persisted field changes.

# Notes

[watermelon-full-preview] Lane mu-pidfdenoent, nested worktree
`.muse/worktrees/pidfdenoent`. Number 2027 reserved by the coordinator
(the same brief's trailing "1995" line names an unrelated, already-filed
Muse delivery issue and is not this work). Companion to #1938
agentctl-pidfd-open-einval on the older kernel. No lane, host or person
is named here.
