---
title: 'agentctl-cancel-resurrection: a cancelled entry is resurrected as pending by a blocked sender'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-20T20:46:00-05:00
updated_at: 2026-11-20T20:46:00-05:00
---

# Description

[watermelon-full-preview] A queue entry that was cancelled (in
`cancelled/` with `delivery_state: cancelled`) reappeared as
pending in `inbox/` 26 minutes later, written by a sender holding
the same request id whose readiness wait had blocked ("did not
become idle/done within 60s"). The prompt it carried had already
been delivered by hand, so the resurrection risks typing it again
as a duplicate. The move to `cancelled/` left no tombstone behind
(a hand move), so writers that only honour tombstones never saw
the retirement.

# Design

- Every writer of an inbox entry checks, under the queue lock,
  whether the id is retired — a tombstone in `cancel/` or a copy
  already in `cancelled/` — and refuses to resurrect it: a blocked
  write-back is skipped, and a return-to-inbox from `inflight/`
  sets the item aside in `cancelled/` instead.
- Drains skip ids that have tombstones (already true at the top of
  the drain loop); a pending copy whose id is already in
  `cancelled/` is set aside untyped, never delivered.
- The existing readiness and not-staged write-back guards (#1775
  (c)) keep working; this closes the remaining writers (attempt
  bound, input-stalled, batched revert, and the composer-retry
  return) and the no-tombstone hand move.

# Acceptance Criteria

- A test where a blocked sender wakes after its prompt was
  cancelled (with and without a tombstone) leaves the item in
  `cancelled/`, nothing in `inbox/`, and types nothing.
- A drain facing a pending copy whose id is already in
  `cancelled/` sets it aside and delivers nothing for it.
- Retire and cancel keep their current behaviour; the focused
  queue and drain suites pass in both editions where they apply.
- (Fold-in) A send whose explicit id already exists in a terminal
  phase is refused as a duplicate, never re-enqueued (pinned for a
  cancelled named id); `cancel` on an id in several phases keeps
  the terminal record and retires the pending copy instead of
  refusing with "exists in multiple queue phases".

# Notes

[watermelon-full-preview] Filed for lane mu-musedeliver with
#1995 agentctl-muse-working-delivery (coordinated by coordb);
evidence was read-only from another team's queue, never written.
Related: #1775 agentctl-drain-composer-wait-labelled-lock-wait.
No lane, host or person is named here.
