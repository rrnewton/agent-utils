---
title: "rust-status-consistency: Rust status/doctor check registry identity across the read, and audit judges rows in parallel"
status: closed
priority: 3
issue_type: feature
assignee: cl-census
created_at: 2026-11-10T10:00:00.000000000+00:00
updated_at: 2026-11-10T15:30:00.000000000+00:00
---

# Description

Two Rust-edition gaps left after #1172 audit-lock-hold:
1. Consistency. Rust status and doctor read the registry lock-free, with no before/after identity check, so a
   report can mix two instants if a mutation lands mid-read. `global_state.rs` notes that the lock-based
   archive/active overlap re-read the Python reader performs is not reproduced. Python (#1172) now measures
   outside the lock and checks a content identity: digests of every control file except `*.lock`, plus per machine
   the complete-event count, the last event's digest and partial names, quarantine journals and
   `slot-images/*/*/IMAGE.json`. On a mismatch it re-measures the changed rows.
2. Parallel audit. Rust audit accepts `--jobs` (validated as at least 1) but judges registered rows serially. Python
   uses a small pool (default min(4, CPUs)), which took the fixture's registered-rows phase from 27.4 s to 11.4 s.

# Design

- Compute the same content identity before and after the Rust status/doctor read; on a mismatch, re-measure the
  changed rows (or retry the read a bounded number of times), matching Python's output.
- Give Rust audit a bounded worker pool honouring `--jobs`, with the same default and the same output order.

# Acceptance Criteria

- A differential where a mutation lands mid-read: both editions produce a consistent report (re-measured), never a
  mixed one.
- Rust audit with `--jobs 4` produces output identical to `--jobs 1` on a fixture, and is measurably faster on the
  #1172 fixture.

# Notes

Filed for the lead coordinator (2026-11-10) from #1172's Rust parity report (items 3 and 4). Rust audit already has
the #797 per-row revalidation.

## Resolution

Both gaps closed in the Rust edition (cl-census, opus 5.5).

1. Consistency. `global_state::registry_read_identity` ports the Python `_registry_read_identity`: digests of every
   control file except `*.lock`; per `EVENTS.*` directory the complete-event count, the last event's digest and the
   partial names; quarantine `*.journal`; and `slot-images/*/*/IMAGE.json`. `global_state::consistent_read` runs
   `status-text` and `doctor-text` lock-free between two identity reads, and publishes a report (or a refusal) only
   when the identity did not change across it. Otherwise it measures again. After `PHASED_READ_RETRIES` (2) repeats,
   the last measurement holds the shared `ACTIVE.global.lock`
   (`ensure_states::acquire_shared_lock` / `flock_wait_shared`, added beside `flock_wait`, which is unchanged), as the
   Python phased read does. Python re-measures only the changed rows; Rust repeats the whole read, and the report is
   the same. The writer turnstile that Python readers also enter is #1199's.
   Differentials `test_status_publishes_the_registry_after_a_change_mid_read_like_python` and
   `test_doctor_publishes_the_registry_after_a_change_mid_read_like_python` land a row change after the first
   measurement (test seam `WRKSLOTS_TEST_REGISTRY_CHANGE`). The Rust report shows the changed row and equals Python's
   report of the registry as it now is. With the identity check sabotaged, both fail with the stale task. Unit tests:
   `read_identity_tests`.
2. Parallel audit. `audit-text` now passes `--jobs` through. Registered rows are judged by a scoped-thread pool
   (`judge_in_pool`), default min(4, CPUs) as `_AUDIT_DEFAULT_JOBS`, with results in registry order and the first
   refusal in that order winning. The shared process snapshot is a `OnceLock`, still captured only when a row needs
   it. `test_other_edition_pools_rows_with_the_same_report` (the #1172 verdict-mix fixture): `--jobs 4` reports exactly
   what `--jobs 1` does; whole-command wall time 3.06 s at `--jobs 1` against 2.26 s at `--jobs 4` (process start and
   the census dominate at 8 rows).
