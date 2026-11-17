---
title: 'local-ci-hub-receipt-fields: write and judge terminal records under the agreed receipt contract'
status: open
priority: 2
issue_type: task
labels:
- local-ci-hub
created_at: 2026-11-17T20:25:10.000000000+00:00
updated_at: 2026-11-17T20:25:10.000000000+00:00
---

# Description

Step 2b (receipt code only): the runner writes terminal records
carrying the agreed receipt fields, and the receipt predicate
judges them under those rules. The consumer reads these records
with a pinned older reader, so compatibility is part of the work.

Record schema version written: 6. Writers stamp
`schema_version: 6` and include `run_id` (required from schema 6).
The consumer predicate's accepted set for these fields is the
schema-6 set; the local reader accepts the same records and keeps
judging older records under the rules of their own version.

# Design

- The runner stamps identity and admission fields (producer,
  record and run identity, repository, trees, anchoring, tooling,
  admission proof, timing) and the result handling stamps the
  canonical result spelling.
- A project adapter maps validator output to test totals, gate
  accounting, coverage and cell results; project meaning stays
  configuration.
- The predicate refuses unknown result values and, for schema-6
  records, a missing repository; older records keep their era's
  rules, and at schema below 5 gate and coverage clauses are
  absent rather than vacuously passing.

# Acceptance Criteria

- New records carry every agreed field with the agreed types and
  fixed-width UTC finish time; writers emit only canonical result
  spellings.
- Predicate tests cover both sides of each new clause, the
  schema 4/5/6 boundaries, and the cross-repository case.
- Existing ledger and receipt behaviour for older records is
  unchanged.

# Notes

The agreed field table is the receipt-field contract document in
the plans directory. Checkout lifecycle work is a later step;
the sweeper remains attended-only.
