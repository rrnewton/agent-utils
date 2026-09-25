---
title: 'old-checkout-cleanup: land and remove local work'
status: open
priority: 1
issue_type: chore
labels:
- repository
- cleanup
depends_on:
  agent-utils-1: parent-child
created_at: 2026-09-25T03:55:15.315366091+00:00
updated_at: 2026-09-25T03:55:15.315366091+00:00
---

# Description

Preserve and land every useful local commit and uncommitted change from the old checkout, reconcile it with current main, validate it, push a linear history, then leave the old checkout clean and remove temporary worktrees and branches.

# Acceptance Criteria

All useful old-checkout work is landed or proven superseded, origin main contains it, the old checkout is clean, and temporary branches and worktrees are removed.
