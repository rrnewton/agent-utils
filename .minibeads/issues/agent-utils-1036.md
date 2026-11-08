---
title: 'merge-queue-planner-stacking: an --after change is composed off its dependency, and a prioritized stale green stacks under a non-prioritized root'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-08T00:44:37.000000000+00:00
updated_at: 2026-11-08T00:44:37.000000000+00:00
---

# Description

Two planner composition bugs from one consuming project's queue
(run 20261107T153547Z on 98f4b65d), sharing the rule that what a
change is composed on must honour its declared ordering:

- `--after` holds only the landing, not the composition. A change
  submitted `--after X` was composed on another change's candidate
  instead of on X's candidate tip, so it validated without its
  declared dependency: a wasted validation slot, and stale the
  moment X lands.
- A prioritized stale green is stacked behind a non-prioritized
  promoted disjoint root: the stale-green handling pushes it to the
  front to be rebuilt, but it is then stacked on the promoted root's
  tip with no priority comparison, so the prioritized change depends
  on a non-prioritized one's green. Its message also names the base
  while its real parent is the stack head's tip.

# Design

- An `--after X` change is composed on X's candidate tip, or waits
  for X's verdict; it is never composed on an unrelated candidate.
- A prioritized stale green that is disjoint from the head gets its
  own root on the base; it never stacks under a non-prioritized
  head. Stale-green messages name the real parent they rebuild on.
- Reuse the existing root/stacking machinery; no new planner.

# Acceptance Criteria

- A change submitted `--after X` validates on a candidate that
  contains X (or waits), in the order submitted, and lands after X.
- A prioritized stale green disjoint from a promoted non-prioritized
  root validates as its own root on the base and does not wait for
  that root's green; its messages name the base as its parent.
- A non-prioritized stale green, and a prioritized one that is not
  disjoint from the head, keep today's stacking behaviour.
- Existing merge-queue behaviour is otherwise unchanged.

# Notes

Reported 2026-11-07 by a consuming project's merge queue (run
20261107T153547Z); consumers are referred to generically here, per
repository convention. The two are landed together because they
share the planner's composition-parent choice.
