---
title: 'agentctl-send-timeout-inotify-wait: a send runs hours past its timeout on an unbounded wait'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-21T01:57:00-05:00
updated_at: 2026-11-21T01:57:00-05:00
---

# Description

[watermelon-full-preview] A consumer team reports (2026-11-21) an
`agentctl send <agent> ... --timeout N` started from a Muse lane's
shell ran 4h12m (about 12:36 to 4:50 AM ET), far past its timeout,
holding an inotify fd, until the lane killed it. The message had
already been superseded, so the wait served nothing. Waits in the
send path that no deadline reaches are the defect class: in the
Rust edition the identified send enqueues through a plain system
runtime, so its queue-lock wait ignores the send deadline; in the
Python edition there is no overall send deadline at all and its
flock acquisition blocks in the kernel indefinitely.

# Design

- One overall send deadline in both editions, honoured by every
  wait in the path: lock waits (queue, lifecycle, target), the
  enqueue serialisation wait, readiness, and the delivery and
  confirmation waits. No queue format change.
- The Rust identified/managed send wraps its runtime in the same
  deadline runtime the lifecycle lock already uses, so the enqueue
  lock wait ends at the deadline like the drain's waits do.
- The Python send/drain gains `--deadline` (default: the registry's
  send_deadline_seconds, else 600), a bounded flock that polls with
  LOCK_NB until the deadline, and readiness/confirmation waits
  capped by what remains.
- When a send gives up it exits non-zero (75), prints the queue id
  and how to check it (`agentctl status NAME`) or cancel it
  (`agentctl cancel NAME ID`), and leaves the queued item intact:
  not lost, not duplicated.
- Tests use a seam and a fake clock or condition, with no real long
  waits: a held lock plus a clock at the deadline ends the wait.

# Acceptance Criteria

- A Rust send whose enqueue lock is held past its deadline returns
  at the deadline with the prompt pending and its id named.
- A Python send/drain whose queue lock is held past its deadline
  gives up at the deadline the same way; the queued item is intact.
- Normal sends are unchanged when nothing contends.
- Focused suites pass in both editions.

# Notes

[watermelon-full-preview] Filed for lane mu-musedeliver, before
#2011 agentctl-claude-permission-answer (coordinated by coordb).
No lane, host or person is named here.
