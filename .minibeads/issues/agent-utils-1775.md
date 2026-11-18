---
title: 'agentctl-drain-composer-wait-labelled-lock-wait: a composer wait is blamed on a lock, lifecycle lock held across waits, cancelled record resurrected'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-17T14:14:41-05:00
updated_at: 2026-11-17T14:14:41-05:00
---

# Description

[watermelon-full-preview] Follow-up to #1773
agentctl-muse-placeholder-hint-as-draft, from the same stuck seats.

1. Mislabelled wait. Later sends report "cancelled while waiting
   for a lock of queue ...; nothing was typed: cannot invoke Herdr:
   control command was cancelled" (under `timeout 40`), with no
   holder in `fuser` or `/proc/locks` and the item still in
   `inbox/` while status said "staged, nothing pending". The drain
   maps target-lock/readiness Herdr errors through its queue-lock
   label, so a composer wait is blamed on a lock wait.
2. Lifecycle lock held across a composer wait. The blocking lock
   was the lane's lifecycle lock (`REG/locks/<name>.lock`), held by
   `agentctl watch-agents` while it waited on the occupied composer
   (notice delivery). That blocked drain (345 s), read and
   repair-watch for that lane until a person cleared the composer.
3. Cancel race. `agentctl cancel` moved a message to `cancelled/`,
   but a delivery attempt already holding the record wrote it back
   into `inbox/` as pending, so it sat in both phases ("exists in
   multiple queue phases") and the watcher reported it held.

# Design

- The drain error names the real wait: composer occupied (with the
  draft's first 40 chars), Herdr readiness cancelled, or a real
  lock holder with its PID. Status counts `inbox/` items as
  pending.
- No agentctl path holds an agent's lifecycle lock while it waits
  on a composer: release before waiting, or bound the wait and
  give up with a clear "composer occupied" outcome.
- A delivery write-back re-checks under the queue lock that the
  record is still in `inbox` (or compares a generation) and drops
  its write if it was cancelled.

# Acceptance Criteria

- Test: watch-agents notice delivery meets an occupied composer
  while drain and read run concurrently; neither blocks on the
  lifecycle lock past its bound.
- Test: a record cancelled mid-delivery is never written back to
  `inbox/`; no "multiple queue phases" state.
- A blocked send's error names the composer, not a lock, when the
  composer is the wait; status shows the inbox item pending.
- `make validate` passes on the gated HEAD.

# Notes

Evidence and record copies were held by the coordinator
(queue-race records, `/proc/locks` and fdinfo for the lifecycle
lock). #1773 removed the placeholder wait itself; this issue is
the labelling, locking and cancel-race remainder.

## Resolution

[watermelon-full-preview] Landed 4652106e on github/main (gated
562babe4, full validate, 62 steps pass). (a) A staged pane whose draft
agentctl may not submit now refuses at once as "composer occupied" on
any real readiness wait, in both editions; zero-timeout probes keep
their wording, so no agentctl path waits out a composer behind a
lifecycle or queue lock. (b) The drain's lock-wait label names the
real wait (Herdr, composer, or a genuine lock acquisition), and
status counts inbox.v2 items as pending. (c) Every drain write-back
re-reads the record's phase under the queue lock and drops the write
when it was cancelled, processed or moved; queue recovery never
repends a tombstoned item. Regression tests fail on the base and
pass here, condition-based, in both editions.
