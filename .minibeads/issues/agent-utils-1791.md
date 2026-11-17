---
title: 'wrkslots-clean-caches-report-mutates: clean-caches in report form must not retire proof mirrors'
status: closed
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-17T18:00:00+00:00
updated_at: 2026-11-17T21:05:00+00:00
closed_at: 2026-11-17T21:05:00+00:00
---

# Description

[watermelon-full-preview] `wrkslots clean-caches` in REPORT form still retires idle proof mirrors under the mutation locks (a report run dropped 20, each logged "(idle)"). A report mode must not mutate.

# Design

Make the report form truly read-only: it reports what WOULD be retired, and put retirement behind the explicit apply form (`--only`/`--yes` without `--dry-run`). History check: the retirement call in `_cmd_clean_caches` (from #1692 wrkslots-remove-proof-mirror) is unconditional and predates no report/apply distinction for mirrors; no commit or issue records report-mode retirement as deliberate.

# Acceptance Criteria

A report run (no `--only`/`--yes`, or `--dry-run`) drops no proof mirror and reports the mirrors it would retire; an apply run retires them as before. Deterministic regression test fails before and passes after.

# Notes

Sibling of #1790 wrkslots-self-contained-nested-repo-in-cache-disposable.

## Resolution

[watermelon-full-preview] Landed as 79a6defe (fix 4824ad44) on main. clean-caches retires idle proof mirrors only on an apply run (--only/--yes without --dry-run); report runs name would_retire_proof_mirrors and drop nothing. History check found no deliberate report-mode retirement (the unconditional call came with #1692); coordb was told before the change. Regression test test_clean_caches_report_does_not_retire_proof_mirrors covers report, dry-run and apply forms.
