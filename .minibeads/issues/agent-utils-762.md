---
title: 'agent-teams-backpressure: fail fast with a distinct code when a destination has stuck messages'
status: open
priority: 3
issue_type: task
labels:
- agent-teams
created_at: 2026-11-02T16:26:35.760597235+00:00
updated_at: 2026-11-02T16:26:35.760597235+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). when a destination (team@host or route) has stuck messages (deferred/accepted-not-delivered older than ~15 min, or >=3), new sends to it fail fast with a distinct exit code naming the oldest stuck id and reason (override --queue-anyway); 'accepted' must progress to a delivery receipt within T or alarm; agent-teams status shows per-destination health; watcher escalates to coordinator then owner. Owner wants errors at the sender, not silent outbox piles. Next agent-teams lane after #722 fix lands.
