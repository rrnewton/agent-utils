---
title: 'layout-migrate-moves-known-dirs: an older agentctl moves REGISTRY/restore/ into agents/ as if it were a legacy agent directory'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-13T12:13:52+00:00
updated_at: 2026-11-13T12:13:52+00:00
---

# Description

[opus 5.5] Follow-up to #1359 restore-report-under-agents and #593 registry-agents-subdir. Seen 2026-11-13 ~7:06 AM ET on one host. After `restore --migrate-reports --apply` created `REGISTRY/restore/reports/`, an OLDER agentctl, still at a commit before #1359's b73d2163, moved the whole `REGISTRY/restore/` directory to `REGISTRY/agents/restore/`. Its layout-2 migration treats any unknown top-level directory as a legacy per-agent directory. The report vanished from where new readers look, and `agents/restore` appeared as a bogus agent entry. The registry was repaired by hand.

So a registry shared by agentctl builds of different ages can silently move a NEW known directory into `agents/`. That's data placement going wrong, not just a display issue.

# Design

- The layout migration moves an entry into `agents/` only if it is positively identified as a legacy agent directory (it holds an agent record). It never moves a known registry directory (`restore/`, `locks/`, `archive/`, `bridges/`, `agents/`, ...) or anything unidentified. Unknown entries are reported and left in place.
- Keep the list of known registry directories in one place, shared by the migration, `agentctl list` and `doctor`.
- `restore --migrate-reports` help and the #1359 Resolution note: run it only after EVERY agentctl that touches the registry is at b73d2163 or later.
- Optionally, a registry layout-version marker that older builds refuse to downgrade.

# Acceptance Criteria

- A test: a registry with `restore/reports/<boot>.json` plus a legacy agent dir; the layout migration moves only the agent dir and leaves `restore/` in place.
- A test: an unknown top-level directory without an agent record is reported and not moved.
- The help text carries the version warning.

# Notes

Numbered from devbig030's interim block (1400-1469). Older builds already deployed can't be fixed retroactively; the version rule is the mitigation.
