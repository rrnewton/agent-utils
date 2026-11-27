---
title: 'agent-teams-status-held-reason: the sender sees "pending" while the receiver has held the message with a reason'
status: open
priority: 2
issue_type: bug
labels:
- agent-teams
created_at: 2026-11-26T19:35:00-05:00
updated_at: 2026-11-26T19:35:00-05:00
---

# Description

When the receiving host accepts a team message but its deliverer holds it (for example, the target pane's composer already holds unsubmitted text, so nothing is typed), the receiver's ledger entry carries a `held` object with `since`, a `reason` and `checked_at`. The sender's `agent-teams status ID` shows only `accepted -> TEAM@HOST`, `delivery_outcome: pending` and the route events, even after a later `receipt answered`. On 2026-11-26 the lead coordinator chased an unanswered request for hours. All that time the receiver knew why: every message to that team since 2026-11-24 was held behind an owner draft in a pane that was not even the team's working coordinator (#2325). Messages then pass their time to live while still showing `pending`.

# Design

- The receipt the sender fetches carries the receiver's `held` object (since, reason with the draft text cut to its first 40 characters, checked_at) and the deliverer's target (queue or pane name).
- `agent-teams status` prints `held at the receiver since T: <reason>` and, past `not_after`, `expired undelivered (held)`; `outbox` marks held entries; `--json` adds the fields.
- The receiving host's alarm sends one receipt-style notice back to the sending coordinator when a hold lasts past 30 minutes (one per message, never repeated).

# Acceptance Criteria

- A test with a receiver whose deliverer holds a message: the sender's status shows the held line and reason; the JSON has the held object.
- A held message past its time to live shows as expired undelivered, not pending.
- The 30-minute held notice reaches the sender once.

# Notes

Found in the #2108 debugging pass (owner request 392, 2026-11-26). Companion to #2325.
