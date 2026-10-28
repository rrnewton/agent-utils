---
title: 'gate-flake-graph-wrkslots: a graph test and the wrkslots shards time out under host load'
status: open
priority: 2
issue_type: task
labels:
- validation
depends_on:
  agent-utils-633: discovered-from
created_at: 2026-10-28T13:58:39.524556306+00:00
updated_at: 2026-10-28T13:58:39.524556306+00:00
---

# Description

[opus 5.5] Moved from the coordinator's private backlog by #633 backlog-to-tracker (entry dated 2026-10-27). Five landing gates in a row failed the same graph test (case 42) and the wrkslots pytest shards on timeouts; each passed when rerun alone. Wanted: find whether the budgets are wall-clock limits that host load can exceed, and either measure CPU time, resize the shards, or raise the ceilings with a stated reason, so a loaded host no longer fails them. Related: #389 gate-flakes and #472 gate-flake-hardening (both closed).
