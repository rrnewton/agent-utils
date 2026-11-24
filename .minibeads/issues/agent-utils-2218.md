---
title: 'agentctl-restore-legacy-report-scope: a registry never reads another registry''s legacy shared restore report as its own'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- restore
- watch
created_at: 2026-11-24T13:28:00-05:00
updated_at: 2026-11-24T13:28:00-05:00
---

# Description

Restore reports used to be written to one shared host directory, `$XDG_STATE_HOME/agentctl/restore/<boot>.json`, for every registry (before #975 and #1359). Readers still fall back to that directory until `LEGACY_REPORT_SUPPORT_ENDS` (`legacy_report_directories`). When a registry has no report of its own for the current boot, `boot_report_state` takes the shared one, even if a different registry's restore run wrote it.

Seen on 2026-11-24. On a host up since 2026-11-02, one team's `agentctl watch` reports, every cycle, "restore report for this boot is incomplete: coordinator refused: record has no native conversation ID". The report is the shared-directory file from 2026-11-02, written by another project's registry, with 7 refusals. None of the 8 agents it names exists in the watching registry. The watching team can do nothing about it: the agents aren't theirs, and forcing a restore would be wrong. The alarm only became visible with the incomplete-report wording, but the misattribution is older.

# Design

- A report from a legacy SHARED location counts for a registry only if it can be proved to belong to that registry. Prefer a registry identity field, where one exists. Otherwise all of the report's agent names must exist in this registry's records, and its coordinator must be this registry's designated coordinator. A report that can't be proved is ignored for this registry; at most, `restore --dry-run` mentions it once as foreign.
- New reports already live under `REGISTRY/restore/reports/`. Make sure every writer records the registry path, or a stable registry id, in the report, so attribution never relies on name matching.
- Reports from a registry's own legacy locations (`REGISTRY/restore-reports/`, `REGISTRY/agents/restore-reports/`) keep their current meaning.

# Acceptance Criteria

- Test: registry A has no report for this boot, and the shared directory holds an incomplete report naming only agents unknown to A. A's watch restore check doesn't report it as incomplete, and doesn't count it as completed either. It falls through to A's normal no-report handling.
- Test: a shared-directory report whose agents and coordinator all match registry A still counts for A, so legacy behaviour for single-registry hosts is kept.
- Test: a new report written by restore carries the registry identity, and a second registry ignores it.

# Notes

Related: #975 (boot-restore retry, shared state directory), #1359 (reports under the registry), #1405 (restore epochs), #1426 (unprovable pre-epoch report not alarmed).
