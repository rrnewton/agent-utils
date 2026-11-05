---
title: 'agent-teams-delivery-outcome: a sender sees each message''s final delivery outcome'
status: open
priority: 1
issue_type: feature
assignee: '014: mu-loss'
created_at: 2026-11-05T17:43:33.971221827+00:00
updated_at: 2026-11-05T17:43:33.971221827+00:00
---

# Description

An agent-teams sender sees "accepted" when its message is queued, and nothing after. On 2026-11-05 the lead's messages were accepted, then ended in the recipient coordinator's failed/ queue as possibly_submitted, and the lead learned of it only six hours later by asking. The sender needs the final delivery outcome.

# Design

- `agent-teams status <id>` reports the message's final delivery outcome on the recipient side: queued, delivered (with evidence where available, per #896), possibly_submitted, or failed, with the reason and time. Cross-host messages get it through the existing serve path.
- When a message lands in the recipient's failed/ (or possibly_submitted), the sender gets a notice through agent-teams: "<id> to <recipient>: <outcome>: <reason>".
- The outcome is recorded in the sender's ledger entry, so `agent-teams` history shows it.

# Acceptance Criteria

- Test: a message delivered normally shows as delivered in `agent-teams status`. A message forced into failed/ shows as failed, and the sender receives a notice.
- Test: a cross-host message's outcome reaches the sending host.
- Help and the userguide are updated.

# Notes

Requested by the lead on 2026-11-05 at 12:43 PM ET (P1, together with #903), after the lost lead messages of that morning. Moved to the lead team's Muse lane mu-loss (014) on 2026-11-05 at 12:44 PM ET, the lead's call.
