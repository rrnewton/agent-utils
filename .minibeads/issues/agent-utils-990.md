---
title: 'wrkslots-audit-wrong-row-unknown: audit stamps unchanged rows UNKNOWN when host churn discards its census'
status: closed
priority: 1
issue_type: bug
assignee: mu-flakes
created_at: 2026-11-06T22:15:00.000000000+00:00
updated_at: 2026-11-06T23:20:00.000000000+00:00
closed_at: 2026-11-06T23:20:00.000000000+00:00
---

# Description

`wrkslots audit` measured a row against a whole-host process census. When
host process churn discarded every census attempt set (a generation changed
or vanished while it was read), the census fail-closed as indeterminate and
every otherwise-clean row was stamped UNKNOWN, including rows whose own
records never changed. Seen as
test_audit_own_record_change_marks_only_that_row_unknown failing only for
the unchanged slot under parallel load (4/40, 2/48, 1/46 probe runs), and
in first-run gates under #943 gate-flake-wrkslots-proc-scan.

# Design

Fail-closed is correct when a census cannot close. The defect is treating
one discarded attempt set as a closed census: retry with a fresh snapshot
inside a bounded audit census window; only a census that still cannot
close keeps the indeterminate error. Per-PID handling inside the census
(a vanished representative is skipped; genuinely unreadable live evidence
still refuses; evidence-changed still discards the attempt) is unchanged.

# Acceptance Criteria

- A census discarded once by churn is retried and the audit completes;
  unchanged rows keep their measured verdicts.
- A census that still cannot close inside the window stays fail-closed
  with the same error and UNKNOWN stamps.
- Deterministic regression test injecting one discarded census.

## Resolution

_cmd_audit retries the census with fresh snapshots inside a 120 s window
(_AUDIT_CENSUS_WINDOW_SECONDS); the fail-closed error path is unchanged.
Deterministic test: test_audit_retries_a_census_discarded_by_host_churn
injects one discarded census and requires the retry (fails pre-fix).
Parallel-load probe after the fix: 48/48 passes of the own-record audit
test (was ~10% failures). A mount-census test records that a vanished
representative is skipped while live unreadable evidence still refuses.
