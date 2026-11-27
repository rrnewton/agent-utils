---
title: 'validate-coverage-report-on-red: validate prints its cross-coverage report only when the DAG passes, so a red row never shows what was skipped or measured'
status: open
priority: 2
issue_type: bug
labels:
- validation
- cross
created_at: 2026-11-27T16:25:00-05:00
updated_at: 2026-11-27T16:25:00-05:00
---

# Description

`scripts/validate.py` `main` runs the flattened DAG and then judges cross-language coverage and per-case timings:

```python
        code = run(selected, components, ...)
        if code != 0:
            return code
        coverage_state = cross_coverage_report(Path(coverage), expected)
        timing_lines, timing_problems, timing_missing = case_timing_report(...)
```

When any step fails, `main` returns before it reads the coverage records. The records live in a `TemporaryDirectory`, so they are gone when the run ends. A red gate row therefore never shows:

- which cross checks were skipped, and of which kind;
- which skips were measured elsewhere (after #2388 gate-full-partial-delegated-live-scope: the "delegated-live-scope skips measured by the live-scope node" lines);
- which cross nodes left no record.

Seen on the #2388 gate on 668e988f (1795799002114893727-6977b0d9). `cross.dagrun.live-scope` passed, and both delegated nodes passed with the same seven delegated-live-scope labels. Six unrelated steps failed, so the row could not show the seven skips counted as measured. The evidence had to be put together by hand from three step logs.

A red row is exactly when a reviewer most needs to know what the green steps covered: it decides whether a solo rerun of the failed steps is enough.

# Design

- After `run` returns non-zero, still call `cross_coverage_report` (and `case_timing_report`) on the records the run left. Print their lines under a heading that says the run failed and the coverage is informational. Then return the DAG's code unchanged: the report never turns a failure into a pass, and never changes the exit code.
- On a red run a cross node may have left no record because it failed, was cancelled, or never started. Say so in the report: name the node and its dagrun outcome where validate knows it, instead of the plain "no record" line a green run would treat as a gap.
- Keep the green-run behaviour exactly as it is: the same lines, the same PARTIAL demotion, the same exit codes.

# Acceptance Criteria

- A validate run whose DAG fails prints the cross-coverage section (skipped checks by kind, the measured-by lines, nodes with no record), marked as informational on a failed run, and exits with the DAG's own code.
- Tests in py/tests/test_cross_skip_accounting.py cover a failed DAG with coverage records: the report is printed, and the exit code is the DAG's. One test covers a node that failed before writing a record.
- A green run's output and exit codes are unchanged (the existing tests pass untouched).

# Notes

- Found by cl-shardcap on the #2388 gate row (coordb asked for this follow-up).
- Related: #2388 gate-full-partial-delegated-live-scope.
- Frequent-red candidates seen on the same row, for cl-tmpfix's audit annex:
  - agentctl `queue_retire::tests::dry_run_lists_without_moving_and_all_pending_takes_every_lane` (queue_retire.rs:221);
  - wrkviz `summarize_core::tests::claude_backend_receives_each_callers_own_schema` (ETXTBSY starting its fake backend).
