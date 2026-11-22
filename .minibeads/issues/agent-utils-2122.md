---
title: 'wrkslots-hold-tolerates-registration-drift: hold succeeds when a slot has an undeclared nested worktree'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-22T12:05:00-05:00
updated_at: 2026-11-22T12:05:00-05:00
---

# Description

wrkslots hold refuses (rc 3) on live slots that contain a nested Git worktree of the same source repository which the slot row does not declare: the source repository registers a worktree inside the managed agent root that no active checkout row names. A hold is the protective direction, so refusing it on an unrelated registration inconsistency fails dangerous. adopt-checkout also does not cover same-repository nested worktrees.

# Design

hold records the hold even when such registration drift exists, and reports the drift as a warning naming the path; refusals that would make a hold unsafe stay. Separately, give adopt-checkout (or a sibling) a way to declare a same-repository nested worktree inside a slot.

# Acceptance Criteria

- Tests: hold on a slot with an undeclared nested same-repository worktree succeeds with a warning; the slot is then never attempted for removal; adopt covers the nested case or the issue says why not.

# Notes

Reported by a consumer's lead; filed by the coordinator.
