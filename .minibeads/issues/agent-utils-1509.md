---
title: 'local-ci-hub-missing-evidence-root-cause: find why complete manifests read as missing evidence'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-14T02:42:14.070298365+00:00
updated_at: 2026-11-14T02:42:39.606293740+00:00
closed_at: 2026-11-14T02:42:39.606293489+00:00
---

# Description

Consumer champion condition 1 (ACCEPT-WITH-CONDITIONS on Rust ledger M1): 1327 missing-full-evidence verdicts on COMPLETE manifests need a field-level root cause, classified fixed/intended.

# Design

Recompute oracle qualified population and Rust first-refusal reasons (20-commit sample, then full population); compare each clause against the authority predicate; fix defects in verdict.rs with regression tests, document intended differences.

# Acceptance Criteria

Cause -> count -> fixed/intended table over all 1327; defects fixed with maintained tests and stricter count re-measured; intended causes documented with authority backing.

# Notes

## Resolution

[watermelon-full-preview] Root cause found by diffing the oracle's qualified rows against the Rust predicate's first-refusal reason per row — first on a 20-commit sample, then over the full population (1929 oracle-qualified commits; stricter total reproduces the champion's 1564 exactly). Counts are commits, pre-fix:

| Cause | Count | Fixed/intended |
|---|---|---|
| Schema-10 in-row `validation_evidence` check | 1079 | fixed |
| `passed != executed` (incl. absent `passed_tests`) | 210 | intended |
| Base-evidence field missing/invalid | 135 | intended |
| Profile/selection not full | 85 | intended |
| Qualifying row latched by a genuine failure | 29 | intended (veto family) |
| Coverage lists unsatisfied | 17 | intended |
| Commit not anchored | 9 | intended |

The defect: `verdict.rs` refused every schema-10 row lacking an in-row `validation_evidence` field, but the authority reads schema-10 native evidence from an external checker over the evidence archive — fields inside the row never satisfy it, and 0 of 1355 qualified schema-10 rows carry the field. That is a skipped I/O clause misimplemented as a field-presence check. Fixed by removing the refusal and documenting the clause as skipped (looser), with the regression test `schema10_row_qualifies_without_in_row_native_evidence`.

Every other cause is a clause the authority predicate also enforces (count equality with its git-resolved known-gap widening documented as skipped-stricter; all four base fields from schema 5; full profile/selection and anchored/clean-tree requirements; per-node coverage lists; the genuine-failure latch whose green-flaky supersede escape is documented unimplemented). Those are intended differences from the narrower qualified-rows timing view, not defects.

Post-fix re-measurement on the same population: stricter drops 1564 -> 565 (latched 130, passed 210, base 135, profile 64, coverage 17, anchored 9), all intended. Crate tests green.
