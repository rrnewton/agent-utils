---
title: 'agent-teams-seq-backups: keep seq.json backups in a bounded subdirectory, not beside fixed state'
status: open
priority: 3
issue_type: bug
created_at: 2026-11-05T00:25:31.010411567+00:00
updated_at: 2026-11-05T00:25:31.010411567+00:00
---

# Description

Owner rule: no O(N) collections next to fixed state. agent-teams' state directory (`~/.local/state/agent-teams`) holds about 1,100 `seq.json.<timestamp>` backup files beside its fixed state files (1,094 on one host, a similar count on the other). Every sequence write leaves another one, and nothing bounds them.

# Design

- Keep sequence backups in a subdirectory (for example `backups/seq/`) with a retention bound: the newest N, or the last D days.
- Migrate existing `seq.json.*` files into it on first run, keeping only what the bound allows.
- Audit the state directory for other unbounded per-event files and apply the same rule.

# Acceptance Criteria

- Tests: a write keeps at most N backups in the subdirectory; the top level holds only fixed files; existing strays are migrated.
- Gate passes.

# Notes

P3, owner rule via the lead (2026-11-04). No lane yet.
