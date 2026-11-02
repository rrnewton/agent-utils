---
title: 'agent-teams-relay-fallback-route: fall back to the relay instead of expiring messages when a route is down'
status: open
priority: 3
issue_type: task
labels:
- agent-teams
created_at: 2026-11-02T16:26:35.790042720+00:00
updated_at: 2026-11-02T16:26:35.790042720+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). while ssh-030 was down (21:36Z-04:37Z 11-01) 11 lieutenant replies + 1 a consumer team send EXPIRED (2 h TTL) instead of falling back to the relay; teams.json routes are per host (no relay-coord), and adding relay-coord host-wide would also route a consumer team (not on the relay). Need per-team route lists or relay-as-fallback for relay-enabled teams; consider longer TTL for replies. Pair with relay listen.
