---
title: 'agent-teams-state-backups: move relay state backups out of the live state directory'
status: open
priority: 3
issue_type: task
labels:
- agent-teams
created_at: 2026-11-02T16:26:35.779329294+00:00
updated_at: 2026-11-02T16:26:35.779329294+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). ~/.local/state/agent-teams/relay/ accumulates alarms.json.<ts> / quarantine.json.<ts> backup copies beside the live files (O(N) mixed with fixed state, against the owner's rule); move rotations under a backups/ subdir with a cap. (2026-11-01)
