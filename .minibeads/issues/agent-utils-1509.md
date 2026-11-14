---
title: 'local-ci-hub-missing-evidence-root-cause: find why complete manifests read as missing evidence'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-14T02:42:14.070298365+00:00
updated_at: 2026-11-14T02:42:14.070298365+00:00
---

# Description

Consumer champion condition 1 (ACCEPT-WITH-CONDITIONS on Rust ledger M1): 1327 missing-full-evidence verdicts on COMPLETE manifests need a field-level root cause, classified fixed/intended.

# Design

Recompute oracle qualified population and Rust first-refusal reasons (20-commit sample, then full population); compare each clause against the authority predicate; fix defects in verdict.rs with regression tests, document intended differences.

# Acceptance Criteria

Cause -> count -> fixed/intended table over all 1327; defects fixed with maintained tests and stricter count re-measured; intended causes documented with authority backing.
