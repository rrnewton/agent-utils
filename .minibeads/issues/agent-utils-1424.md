---
title: 'watch-restore-legacy-report-path: the watch restore check misses a completed report left under agents/restore-reports/'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-13T11:08:43+00:00
updated_at: 2026-11-13T11:08:43+00:00
---

# Description

[opus 5.5] Follow-up to #1359 restore-report-under-agents (b73d2163). #1359 moved new restore reports to `REGISTRY/restore/reports/` and says restore, the watch restore check and restore-teams still READ the legacy locations (`restore-reports/`, `agents/restore-reports/`, the shared XDG location) until 2027-02-12.

Seen 2026-11-13 ~6:15 AM ET on one host: after its `agentctl watch` unit moved to a build containing #1359, watch alerted "restore <boot>: no completed restore report for this boot", although a complete report for that boot (`complete: true`) sat at `.agentctl/agents/restore-reports/<boot>.json`. Running `restore --migrate-reports --apply` moved it to `restore/reports/` and the alert cleared. So the watch restore check doesn't read `agents/restore-reports/`, or reads it with the wrong name or schema, while restore itself does.

# Design

- Make the watch restore check use the same candidate-path list as restore (one shared function), including `agents/restore-reports/` until the legacy cutoff, and the same file-name and epoch matching.
- Keep `agentctl list`'s skip of recordless `agents/` entries as it is.

# Acceptance Criteria

- A test: a completed report for the current epoch exists ONLY under `agents/restore-reports/`, and the watch restore check finds it (no alert). The same for `restore-reports/`.
- A test that watch and restore pick the same report when several locations hold one.
- Rust and Python editions agree, if both implement the check.

# Notes

Numbered from devbig030's interim block (1400-1469). Registries that ran `restore --migrate-reports --apply` are unaffected.
