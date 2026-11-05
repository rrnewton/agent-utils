---
title: 'pr-landing-planner-retirement: fold the advisory planner into merge-queue plan --advisory'
status: open
priority: 2
issue_type: task
created_at: 2026-11-05T15:50:19.207971866+00:00
updated_at: 2026-11-05T15:50:19.207971866+00:00
---

# Description

Retire `pr-landing-planner` into a `merge-queue plan --advisory` mode, following the design note `ai_docs/transient/2026-11-05-pr-landing-planner-retirement.md` (751fbcc2).

**Status (2026-11-05):**
- The known consumer team has approved the direction, with the conditions recorded below.
- The owner's go to build is pending.
- Filed only: unassigned, not started.

# Design

As in the design note: the advisory planner's plan/graph/cluster/status output moves into `merge-queue plan --advisory`. `pr-landing-planner` becomes a compatibility shim, then is removed in a final step once consumers have moved.

# Acceptance Criteria

The consumer team's conditions:

1. **Fixture shape is pinned:**
   - `summary{open, passed, failed, no_result, real_reds: int, outage_suspected: bool}`;
   - `prs[]{pr, ci, red_class, draft, labels, title}`;
   - the value sets of `ci` and `red_class` are documented and tested.
2. **Contract points hold:**
   - the exact argv the consumer passes (it passes neither `--landing-context` nor `--flaky-signatures`);
   - stdout is exactly one JSON document;
   - exit code 0 means success;
   - the existing error markers keep their meaning: the sandbox/security-policy refusal markers mean "repository unavailable", and "504" / "changed during collection" mean retry, up to 3 times;
   - the time budget is 300 s per repository and 480 s overall;
   - the shim runs from the Python entry `py/bin/pr-landing-planner`, with no `rustc` and no first-use build.
3. **Removal waits:** the removal step waits until the consumer has updated its setup check (a pinned skill symlink) and its labels configuration.
4. **No hidden dependency:** the consumer does not depend on `refs/pr-landing-planner/*`, so the shim and the new mode need not keep those refs.

# Notes

Filed at the lead's request on 2026-11-05 at 10:49 AM ET. The consumer team's detailed review with file:line references is held on the lead host. Unassigned until the owner says build.
