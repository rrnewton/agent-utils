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
updated_at: 2026-09-25T04:18:56.178713374+00:00
claimed_at: 2026-09-25T04:17:03.192733505+00:00
claimed_until: 2026-09-27T04:17:03.192544933+00:00
---

# Description

Preserve and land every useful local commit and uncommitted change from the old checkout, reconcile it with current main, validate it, push a linear history, then leave the old checkout clean and remove temporary worktrees and branches.

Also audit every registered agent-created worktree and branch visible from the old checkout. Remove worktrees and delete branches only after proving their useful work is in origin/main or shepherding unique intended work through validation and landing; preserve ambiguous human-owned branches, prune stale missing-directory registrations, and retain the durable recovery bundle until the complete cleanup is verified.

# Acceptance Criteria

All useful old-checkout and registered agent-worktree work is landed or proven superseded; origin/main contains it; the old checkout is clean; merged agent branches and worktrees plus stale registrations are removed; ambiguous human-owned branches are preserved; no temporary reconciliation branch/worktree remains; and the durable recovery bundle is retained through final verification.
