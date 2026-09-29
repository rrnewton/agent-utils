---
title: 'old-checkout-cleanup: land and remove local work'
status: in_progress
priority: 1
issue_type: chore
assignee: gpt-6
labels:
- repository
- cleanup
depends_on:
  agent-utils-1: parent-child
created_at: 2026-09-25T03:55:15.315366091+00:00
updated_at: 2026-09-29T20:13:18.635140250+00:00
claimed_at: 2026-09-25T04:17:03.192733505+00:00
claimed_until: 2026-09-27T04:17:03.192544933+00:00
---

# Description

Preserve and land every useful local commit and uncommitted change from the old checkout, reconcile it with current main, validate it, push a linear history, then leave the old checkout clean and remove temporary worktrees and branches.

Also audit every registered agent-created worktree and branch visible from the old checkout. Remove worktrees and delete branches only after proving their useful work is in origin/main or shepherding unique intended work through validation and landing; preserve ambiguous human-owned branches, prune stale missing-directory registrations, and retain the durable recovery bundle until the complete cleanup is verified.

# Acceptance Criteria

All useful old-checkout and registered agent-worktree work is landed or proven superseded; origin/main contains it; the old checkout is clean; merged agent branches and worktrees plus stale registrations are removed; ambiguous human-owned branches are preserved; no temporary reconciliation branch/worktree remains; and the durable recovery bundle is retained through final verification.

# Notes

[gpt-5] Read-only cleanup audit on 2026-09-29, against current `origin/main`:

- 13 worktrees are registered. Four are clean and point to commits already in main. Nine are not
  ancestors of main; seven are clean and two contain modifications, so none of those nine is safe
  to remove merely as stale registration.
- The nine unmerged worktrees reduce to five patch identities. The normal reply-burst patch is
  already in main as `30d7e315`; the exact-boundary retry, an alternate reply-breaker patch, and
  the two-commit chat-bridge stack are not patch-equivalent to anything in main.
- The old checkout's sole untracked issue comment is superseded: current main tracks the same
  content plus a later acceptance record. Its branch is 96 commits beyond its configured local
  origin, however, so that checkout still needs a deliberate history reconciliation before it can
  be removed.

No worktree, branch, recovery reference or bundle was deleted in this audit. The remaining action
is destructive and needs an owner decision about the unmerged stacks and the two dirty negative-
control/review worktrees.
