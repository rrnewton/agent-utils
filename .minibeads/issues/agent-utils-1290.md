---
title: 'gate-result-delivery: a finished gate verdict reaches its lane and the coordinator'
status: open
priority: 1
issue_type: feature
assignee: unassigned
created_at: 2026-11-11T12:15:00+00:00
updated_at: 2026-11-11T12:15:00+00:00
---

# Description

[watermelon-full-preview] A finished gate's verdict reaches nobody today: a
lane learns its gate finished only by polling its own log, so a lane
that ends its turn (or a session that is compacted or replaced) can
miss the result for hours while its work sits unlanded.

Evidence (coordb, 2026-11-11): mu-effort's full gate (ticket c60804f4)
finished red at 5:16 AM ET and nobody saw it until 7:30 AM ET; the
lane had been idle since 7:30 PM ET the day before. Yesterday at
3:26 PM ET mu-stable lost about 4 hours the same way. Separately,
lanes watched dead gates for hours: mu-docjump's gate log stopped at
10:37 PM ET with no ticket behind it, and nothing flagged that the
gate they were "waiting on" no longer existed.

# Design

- (1) When a ticket finishes, gate.py sends the verdict straight to
  the owning lane and to the coordinator (agentctl send). The
  lane/agent name is recorded in the ticket at enqueue, or resolved
  from the slot registry via the ticket's cwd.
- (2) agentctl watch-agents flags any lane with a finished gate
  verdict it has not acted on within 15 minutes (the lane is idle, or
  still reporting "waiting on gate").
- (3) The watcher, or gate.py at status time, flags a lane reporting
  "waiting on gate X" when no ticket X exists in the queue or runs —
  the gate died or was never queued, and waiting is not a state.
- The finish record already exists (history/ledger); the missing
  piece is delivery and an unacted-verdict alarm, not more recording.

# Acceptance Criteria

- A gate finishing while its lane is idle produces an agentctl
  message to the lane and to the coordinator naming the ticket, the
  verdict, and the failing steps if any; the message arrives without
  the lane polling anything.
- watch-agents flags a finished-but-unacted verdict after 15 minutes,
  and flags a "waiting on gate" claim whose ticket does not exist.
- No message is sent for a ticket whose lane already read/acted on
  the verdict (no duplicate nagging).
- Focused tests: verdict delivery on finish, the 15-minute flag, the
  dead-ticket flag, and ticket lane-name recording at enqueue.

# Notes

Filed by mu-gatefast (issue-only, 2026-11-11) at coordb's request.
Implementation is its own lane later; the design above is coordb's.
The gatefast lane's own experience matches the evidence: several of
its gates finished while this session was compacted or waiting on a
cron backstop, and one queued gate (Part 2, 2026-11-10) died silently
and had to be re-queued by hand.
