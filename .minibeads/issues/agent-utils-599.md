---
title: 'agent-teams-state-subdirs: keep backups and per-agent locks in their own directories'
status: open
priority: 2
issue_type: task
labels:
- agent-teams
- layout
depends_on:
  agent-utils-593: related
created_at: 2026-10-27T22:15:26.822448378+00:00
updated_at: 2026-10-27T22:15:26.822448378+00:00
---

# Description

[opus 5.5] Found by the #593 registry-agents-subdir audit (AGENTS.md rule: a growing collection gets its own directory). rs/agent-teams/src/jsonfile.rs write_json_atomic copies the previous file to <name>.<unix-seconds> beside it on every write, without limit, so the state directory accumulates seq.json.<n> (one per sent message; 17 on one host today) next to seq.json, team-log.jsonl, teams.d/ and outbox/, and ~/.config/agent-teams accumulates teams.json.<n> next to teams.json and routes.d/ (that directory is synced between hosts, so the backups travel too). rs/agent-teams/src/deliver.rs also writes one auto-drain-<agent>.lock per coordinator at the state root.

Proposed fix: backups to backups/<file>/<unix-seconds> with a retention bound (or none for a counter like seq.json), and locks to locks/auto-drain/<agent>.lock; read the old names during a deprecation period, as agentctl does for its registry.
