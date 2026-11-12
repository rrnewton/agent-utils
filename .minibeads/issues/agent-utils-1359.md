---
title: 'restore-report-under-agents: restore reports under agents/ are treated as an unreachable agent'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-12T12:00:00+00:00
updated_at: 2026-11-12T12:00:00+00:00
---

# Description

[watermelon-full-preview] `agentctl restore` reports have been found under `.agentctl/agents/restore-reports/` in a live registry (the forced run after the 2026-11-11 user-manager restart wrote `.agentctl/agents/restore-reports/3a1db123-9323-4661-9213-778427a59ae5.json` there). `agents/` is the layout-2 agent collection: every directory under it is read as one agent, so `agentctl list` and `watch-agents` treat `restore-reports` as a registered agent with no readable record (an unreachable agent), and a listing pass over such a registry can exit 1. Suspected, to confirm or rule out with a fixture registry (never a live one): the 05:30 smoke failure `list (<other registry>) exited 1` on build 78c0288f, whose registry has the same `agents/restore-reports/` directory.

The reports are a growing collection of their own, so under the repository rule that a growing collection gets its own directory, they do not belong inside `agents/` whichever path put them there.

# Design

- New reports default to `REGISTRY/restore/reports/` (outside `agents/`); a configured `report_dir` is unchanged.
- Readers (restore's replay/watch check and `restore-teams`' recorded-report lookup) accept the new location first, then the previous ones — `REGISTRY/restore-reports/`, `REGISTRY/agents/restore-reports/`, and the shared XDG state default — for a stated deprecation period ending 2027-02-12, so a report written by an older build in the same boot or epoch still counts.
- An explicit migrate step (dry run by default, `--apply` to move) moves report files from the old locations into `REGISTRY/restore/reports/`, never overwriting a report already there, and reports what it moved or would move.
- `agentctl list` and `watch-agents` skip an entry under `agents/` that holds no agent record instead of reporting it as an unreachable agent; a directory that does hold a record is still listed (and still errors as its own row when unreadable).
- Confirm or rule out the smoke-failure explanation with a fixture registry holding `agents/restore-reports/<boot>.json` beside real agents.

# Acceptance Criteria

- With a fixture registry, a restore writes its report under `REGISTRY/restore/reports/`, and a completed report left at either old location is still found (replayed by restore in the same epoch, counted complete by the watch check, and found by `restore-teams`).
- The migrate step's dry run changes nothing and names the files it would move; `--apply` moves them, a rerun moves nothing more, and a same-named report at the destination is never overwritten.
- `agentctl list` on a fixture registry with `agents/restore-reports/` and one real agent exits 0, lists the real agent, and does not list `restore-reports`; `watch-agents` rows agree. A fixture record that exists but is unreadable still appears as its own error row.
- The Notes record whether the fixture reproduces the 05:30 smoke failure shape (`list` exiting 1 before the fix).

# Notes

Numbered by the devbig014 lead (2026-11-12); filed from devbig030's lane for #1405 restore-epoch-manager-restart. Follows the migration shape of #593 registry-agents-subdir.
