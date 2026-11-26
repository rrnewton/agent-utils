---
title: 'agentctl-restore-legacy-report-scope: a registry never reads another registry''s legacy shared restore report as its own'
status: closed
priority: 2
issue_type: bug
labels:
- agentctl
- restore
- watch
created_at: 2026-11-24T13:28:00-05:00
updated_at: 2026-11-26T07:45:00-05:00
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

## Resolution

Landed on main in the agentctl stack a724562b..7e89d795 (12 commits replayed by land-main from the gated 62a72cde, the same patch, in the order #2218, #2214, #2222, #2217), as ebc409d7 and 103132a1. Gate ticket 1795671224491121398-107888b7: full selection, 65 steps, pass, finished 7:32 AM ET on 2026-11-26 (rust.agentctl 3106 passed, 0 failed; both python.agentctl shards ran, 1457 passed each).

- Every new restore report records `registry_id` (the per-team key) and `registry_path` (the canonical registry path), at both write sites in `restore()`.
- A report found in the legacy shared directory counts for a registry only when its recorded identity matches. A report from before the identity fields counts only when every agent it names exists in the registry and its coordinator is the registry's coordinator (restore.json or designated). `boot_report_state`, `read_boot_report`, restore-teams' recorded-report read, team verify's snapshot and the team driver's ack clock all apply the same proof.
- `restore --dry-run` names a foreign shared report once, as `foreign_legacy_report`. Reports in a registry's own legacy locations keep their meaning.
- Tests (`rs/agentctl/src/subagents/tests/restore.rs`, `restore.rs`): `a_foreign_shared_report_counts_for_no_registry`, `a_shared_report_matching_this_registry_still_counts`, `a_recorded_registry_identity_decides_shared_attribution`, `restore_reports_carry_the_registry_identity`.
- New persisted fields: `registry_id`, `registry_path` and the dry run's `foreign_legacy_report`, all additive; the consumer whose pinned reader reads restore reports accepted them before landing.
