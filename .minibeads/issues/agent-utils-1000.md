---
title: 'merge-queue-regenerated-paths-not-overlap: a stale green that shares only generated files with what landed should stack'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-07T02:13:01.000000000+00:00
updated_at: 2026-11-07T02:13:01.000000000+00:00
---

# Description

The stale-green exception (run.rs stale_green_stack, the `stale-green-stacked`
event) stacks a candidate that went stale only because unrelated changes
landed. It decides "unrelated" from raw changed files and requires a merge
onto the stack head with no conflict at all. The planner already treats
conflicts confined to `regenerate.paths` as no conflict (plan.rs), because the
combination settles them on the target's side and the regenerate command
rewrites those files on every candidate. The stale-green test does not, so a
shared generated file blocks the exception.

On a queue whose regenerate list holds files almost every change touches
(scorecards, cell tables, a generated validation DAG), nearly every pair of
changes "overlaps", the exception almost never applies, and each landing turns
every other in-flight root into a rebuild. Measured on a consuming project's
queue 2026-11-06 (8:00-9:10 PM ET): one change went green three times (at three
successive bases) without landing, and two concurrent candidates whose only
shared paths were two generated files would rebuild rather than stack.

# Design

Filter both tests in stale_green_stack through RegenerateConfig::covers: a
generated path is not overlap, and a merge that conflicts only in generated
paths counts as clean. Hand-written overlap still rebuilds. No configuration
change; a queue without `regenerate` behaves exactly as before (covers() is
false when regeneration is not configured).

# Acceptance Criteria

- cli_stale_green: a stale green sharing only a generated file with what
  landed stacks (fails on the old code), and one sharing a hand-written file
  (clean merge) still rebuilds.
- USER_GUIDE's stale-green paragraph states the rule.
