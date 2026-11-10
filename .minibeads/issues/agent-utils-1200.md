---
title: "rust-status-consistency: Rust status/doctor check registry identity across the read, and audit judges rows in parallel"
status: open
priority: 3
issue_type: feature
created_at: 2026-11-10T10:00:00.000000000+00:00
updated_at: 2026-11-10T10:00:00.000000000+00:00
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
