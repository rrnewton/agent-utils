---
title: 'agent-teams-state-backups: timestamped backups beside fixed state files (seq, outbox, ledger, relay) move to their own subdirectory with a retention cap'
status: open
priority: 2
issue_type: task
labels:
- agent-teams
created_at: 2026-11-02T16:26:35.779329294+00:00
updated_at: 2026-11-02T16:26:35.779329294+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). ~/.local/state/agent-teams/relay/ accumulates alarms.json.<ts> / quarantine.json.<ts> backup copies beside the live files (O(N) mixed with fixed state, against the owner's rule); move rotations under a backups/ subdir with a cap. (2026-11-01)

## Widened scope (2026-11-02)

[opus 5.5] At the lead's request (filed as "agent-teams-backups-beside-state", folded in here to avoid a duplicate): agent-teams writes timestamped backups next to ALL of its fixed state files under ~/.local/state/agent-teams: about 250 seq.json.<ts> beside seq.json, and .json.<ts> siblings for every outbox and ledger entry, besides the relay alarms/quarantine copies. That breaks the owner rule against O(N) collections mixed with fixed files (2026-10-27). Backups go in their own subdirectory (per state family) with a retention cap; migrate existing siblings.
