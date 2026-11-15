---
title: 'nested-worktree-guard: refuse or auto-adopt nested worktrees of managed repos'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
created_at: 2026-11-15T14:36:00+00:00
updated_at: 2026-11-15T14:36:00+00:00
---

# Description

[watermelon-full-preview] A lane running plain `git worktree add`
inside its slot registers checkouts with no wrkslots row (6 in the
evidence drain, across 2 slots). The parent slot then cannot be
removed until each registration is adopted, moved, or deleted, and
nothing prevents or auto-adopts the nested add at creation time.

# Design

Detect at heartbeat/audit time (already typed as
git-registration-without-row) and give lanes a supported path:
creation hooks or a wrapper refuse nested adds of managed source
repos inside a slot, or `adopt-checkout` runs immediately with the
nesting recorded. Removal keeps refusing unknown registrations.

# Acceptance Criteria

- A nested add inside a slot is refused with the supported command
  named, or adopted with a recorded row in the same operation.
- Existing nested fixtures still resolve via adopt-checkout.
- Foreign (non-managed) nested checkouts keep current behaviour.
