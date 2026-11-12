---
title: 'restore-report-under-agents: restore reports under agents/ are treated as an unreachable agent'
status: closed
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

## Resolution

[watermelon-full-preview] New reports default to `REGISTRY/restore/reports/` (`restore::reports_directory`); a configured `report_dir` is unchanged. Restore, the watch restore check, and `restore-teams` read the current (or configured) location first, then `REGISTRY/restore-reports/`, `REGISTRY/agents/restore-reports/`, and the shared XDG directory, until `LEGACY_REPORT_SUPPORT_ENDS` (2027-02-12). `agentctl restore --migrate-reports` previews the move and `--apply` performs it, never overwriting a report already at the destination and never sourcing from the shared XDG directory (its reports cannot be attributed to one registry). `list` and the watch rows in both editions skip an entry with no agent record in either layout; a record that exists but is unreadable still gets its own error row, so `list` still exits 1 for a genuinely broken record.

Smoke check: with a fixture registry holding `agents/restore-reports/<boot>.json` beside one real agent, `list` now exits 0 listing only the real agent in both editions (Rust `list_skips_a_recordless_directory_under_agents`, Python `test_cli_list_skips_a_recordless_directory_under_agents`). The pre-fix Rust path produced a `record_error` row for `restore-reports` and the CLI maps any such row to exit 1, which matches the 05:30 smoke failure shape (`list (<other registry>) exited 1`) on a registry with that directory.

Gate note (2026-11-12): the full gate's remaining reds are known untouched defects, waived by the lead for this landing. The restore drill (`test_restore_drill_happy_path_fresh_pane_slot_and_rerun`, #1477 restore-drill-happy-path-cpu-capped) fails at `tests/test_agentctl_restore_drill.py:747` (`queued("cheap") == []` after a complete, all-revived restore) in #1359's capped gate, its capped rerun, and its uncapped quiet-host rerun — and identically on a clean `github/main` checkout (8500c1ac38) on this host, so the red is pre-existing here and independent of this change. `test_launch_sigterm_exits_within_a_few_seconds` and the `cross.agentctl.differential-2` CPU-TIMEOUT under the serial memory-capped model are likewise waived as known load defects; differential-2 showed no divergence.

Combined-tree check (2026-11-12, lead's overlap rule): landed as the clean rebase of the gated change onto `github/main` 19faf8a40b (tree 6f34b60fbf before this note; the rebased non-issue diff is content-identical to the gated one). Contained focused tests on the combined tree all passed: `cargo test --lib restore` (104), `list_` (9), `migrate` (5), `revive` (113), `relaunch` (5), clippy clean, `cargo fmt --check`, `pytest py/tests/test_agentctl_sessions.py` (188), and strict mypy on the changed Python files. An uncapped run of `cross.agentctl.differential-1` and `cross.agentctl.differential-2` on the combined tree ended in CPU-TIMEOUT on both with zero DIVERGENCE lines, waived per the lead ruling above.
