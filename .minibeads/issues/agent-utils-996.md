---
title: 'gate-flake-census-churn-abort: census evidence-changed aborts fail liveness tests under host load'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-07T02:20:00.000000000+00:00
updated_at: 2026-11-07T02:20:00.000000000+00:00
---

# Description

Under full-gate host load, whole-host process censuses discard all three
attempt sets to unrelated process churn and refuse "relevant process
evidence changed during three liveness attempts". Tests whose assertions
require a completed census then fail in gates and pass alone:

- test_lifecycle.py::test_process_path_census_reports_a_live_user_while_unrelated_processes_exit_during_it
- test_lifecycle.py::test_validate_complete_still_refuses_live_path_use_after_owner_dies
  (refused with the churn text instead of naming the live process)

The census lane recorded identical failures on clean main when landing
its census scoping. Observed again in both #943 validation runs
(2026-11-07): each failure passes alone in seconds.

# Design

Scope the audit/remove census closure to target-relevant evidence (as
the same-UID census already closes on proven-unrelated rounds), or make
attempt aborts distinguish target-relevant identity changes from
unrelated churn, keeping holder detection fail-closed: a generation
that could hold a target path must still discard the attempt.

# Acceptance Criteria

- Both tests pass 20 consecutive runs under bounded contention
  (stress in a CPUQuota scope, or parallel gate-equivalent load).
- A live holder appearing/vanishing mid-census is still detected
  (existing census holder tests green).

# Notes

Filed under the #943 waiver (coordb, 2026-11-07). The reclaim
item-budget failure from the same gates is tracked by #954, not here.
