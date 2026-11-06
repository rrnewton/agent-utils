---
title: 'wrkslots-validate-census-bound: whole-directory census blocks validate-slot removal'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-06T13:49:24.335470498+00:00
updated_at: 2026-11-06T20:13:07.070920502+00:00
---

# Description

[watermelon-full-preview] The validation-run liveness census in wrkslots refused removal of any finished validate slot when the whole retained run-handle directory exceeded a 256 MiB bound, regardless of which slot was being removed. Finished handles for other rows and service-result sidecars counted toward the bound. The census also rejected current run records carrying service_result_schema 8. Fix: census only canonical handles naming the requested slot or checkout, apply the count/byte bounds to that matched set, and accept schema 8 with its skipped_tests field and outcomes table. A live, ambiguous, or unknown handle naming the requested slot still refuses.

# Acceptance Criteria

A large handle directory with a finished target slot allows removal; a live handle naming the target still refuses; an ambiguous or unknown handle for the target refuses; schema 8 is accepted in removal-record, producer-schema, and service-result projection paths; schemas 1-7 are unchanged; make validate passes on the exact tree.
