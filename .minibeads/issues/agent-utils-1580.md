---
title: 'audit-leftover-classification: distinguish leftover processes from a live lane'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
created_at: 2026-11-15T14:36:00+00:00
updated_at: 2026-11-15T14:36:00+00:00
---

# Description

[watermelon-full-preview] Occupancy correctly counts a deleted
cwd (the process holds the inode), so build daemons and shells
left in deleted directories keep a dead lane reading as live to
every scanner for days. Audit cannot distinguish "agent alive"
from "only leftovers remain", so operators get no kill list.

# Design

Audit classifies occupancy holders: recorded owner/ancestry,
service processes with live cwds, and leftover-only holders
(deleted cwd, no live owner). Leftover-only slots report a named
candidate list (process identities and their supervisors where
known) for the operator to stop through the owning supervisor.
Removal semantics do not change.

# Acceptance Criteria

- Fixture with only a deleted-cwd holder audits as leftover-only
  with the holder named, not as a live lane.
- A live owner or live-cwd service still audits as occupied/live.
- No classification ever authorizes removal by itself.
