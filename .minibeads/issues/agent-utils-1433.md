---
title: 'reap-census-archived-unresolvable: report-only reap census reads past archived rows with deleted source repos'
status: closed
priority: 1
issue_type: bug
assignee: mu-abortfail
labels:
- wrkslots
created_at: 2026-11-15T12:39:56+00:00
updated_at: 2026-11-15T12:39:56+00:00
---

# Description

`wrkslots reap-stray-worktrees --repository <repo>` (report only) refuses before reporting:

    source repository 'ignored/...-source.git' does not exist or cannot be resolved

The cause is ARCHIVED, finished rows that still name deleted source repositories (their checkouts are gone too). One such row blocks the whole census (`py/wrkslots/cli.py`, `_cmd_reap_stray_worktrees` resolving every archived checkout's repository through `_resolved_repository_path`). Separate from #1431 wrkslots-abort-create-failed-hook; this change stacks on it and gates after it.

# Design

- An archived row whose source repository cannot be resolved cannot own any live registration, because the repository it would live in is gone. So in BOTH modes (report and `--apply`) and both editions, the census skips such rows and lists them by name (`UNRESOLVABLE-ARCHIVE: machine=... slot=... repository=... reason=...`; JSON key `unresolvable_archives`). Refusing `--apply` over them would block the real reap forever, for no safety gain.
- An active row whose repository is unresolvable refuses in both modes, as does anything other than an archived row that is ambiguous.
- The Rust edition implements `reap-stray-worktrees` (`rs/wrkslotsd/src/reap_strays.rs`) and stays in step (it previously skipped unresolvable rows silently in both modes; it now names archived ones and refuses active ones), covered by the existing differential suite.

# Acceptance Criteria

- (1) Archived row with a deleted source repository: reported by name and skipped in report mode, and the census completes.
- (2) The same with `--apply`: skipped, named, and the other strays still reaped.
- (3) Active row with a missing repository: refuses in both modes.
- Cross-edition differential coverage for all three.
- Fixtures use the worktree representation only; no real image slots.
- The gate passes at normal priority (after #1431) and the fix lands on `main`.

# Notes

Assigned by the coordinator from the devbig030 ledger block. Stacked on #1431 (414dd77562); gates separately after it.

## Resolution

`_cmd_reap_stray_worktrees` now catches the repository-resolution `Refusal` for archived rows in both modes, records `{machine, slot, repository, reason}` in the new `unresolvable_archives` payload list (printed as `UNRESOLVABLE-ARCHIVE:` lines), and continues the census — including reaping the real strays under `--apply`, since a gone source repository cannot own any live registration. Active rows resolve through the strict loader before the census and refuse in both modes. Tests in `py/wrkslots/tests/test_reap_stray_worktrees.py`: the deleted-repository archived row is named and skipped while the stray is still reported, the same under `--apply` with the stray reaped, and the active-row refusal in both modes. The Rust edition (`rs/wrkslotsd/src/reap_strays.rs`) mirrors the behavior — its reader previously skipped unresolvable rows silently in both modes; it now names archived ones in both modes and refuses active ones — with differential cases in `py/wrkslots/tests/test_reap_stray_rust.py` comparing both editions across all three.
