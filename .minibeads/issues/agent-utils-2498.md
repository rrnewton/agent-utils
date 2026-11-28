---
title: 'nested-git-audit-verdict-deletable: the nested-git policy differential sometimes gets DELETABLE from the audit where the census should leave the expired row UNKNOWN'
status: open
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- wrkslots
- tests
- census
created_at: 2026-11-28T15:30:00-05:00
updated_at: 2026-11-28T15:30:00-05:00
---

# Description

`py/wrkslots/tests/test_validate_nested_git_policy.py::test_audit_and_validation_removal_agree_on_one_tree` builds a validation slot whose cache holds nested Git metadata that reaches outside the cache. It marks the owner dead and asserts that the audit row's verdict is UNKNOWN: what the census found leaves the expired row's state unknown (#1107).

Twice on 2026-11-28 the audit returned DELETABLE instead (`assert 'DELETABLE' == 'UNKNOWN'`), with the session git stand-in on (no fallback):
- the `[core-worktree]` parameter, gate 1795890126740230136-2a0515e3, `python.wrkslots.shard-1`;
- the `[linked-worktree]` parameter, gate 1795894776097312074-bd122e61, `python.wrkslots.shard-1`.

Both passed in solo reruns on the same commits. The `[linked-worktree]` parameter also failed once before the stand-in, in an earlier #2065 gate.

So under some host condition the audit's census does not leave the row UNKNOWN, and the row is judged deletable. The cause is not established.

# Design

- Repro first: run the case repeatedly in its shard's wrapper on a busy host (with process churn, as for #2488). On a failure, capture the census evidence the audit used for that row.
- Name the cause. Then fix the product, if the audit can clear a row it should not, or the test, if its premise depends on host state.
- No wall-clock budget; no new persisted field.

# Acceptance Criteria

- The cause is named from a reproduction.
- The case passes on a busy host in its namespaced shard, in a gate.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28; number pre-assigned), after the second occurrence. The first is noted in #2065.

Related:
- #1107: the audit's unknown state after a census.
- #2065: the hot-host census tests.
