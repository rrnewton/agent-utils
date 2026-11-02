---
title: 'agent-teams-accepted-lost: a send accepted but never settled must be retried or reported'
status: open
priority: 3
issue_type: task
labels:
- agent-teams
created_at: 2026-11-02T16:26:35.729942862+00:00
updated_at: 2026-11-02T16:26:35.729942862+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). a send to agent-utils@a host at 2026-10-31 23:37 ET (heads-up for 5838e39c, file /tmp/vt-headsup-5838e39c.md) returned rc 75 (accepted, not settled) and never arrived; the d89fcb1b one a minute later did. Investigate the outbox / node-exec route retry.
