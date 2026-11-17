---
title: 'wrkslots-remove-locked-foreign-registration-after-delete: check every repository registration before the subvolume delete'
status: open
priority: 0
issue_type: bug
assignee: mu-lockedreg
labels:
- wrkslots
created_at: 2026-11-17T07:00:23.172877567+00:00
updated_at: 2026-11-17T07:00:23.172877567+00:00
---

# Description

remove-validate-batch on a validate slot whose checkout is also registered as a git worktree in a different repository, where that registration is git-worktree-locked, moves the slot to its fenced path, deletes the subvolume, then git worktree prune skips the locked entry and remove refuses after the point of no rollback. The refusal leaves an actor-less finish journal and a validation-batch seal journal that block remove, transfer-owner and validate-run until a hand git worktree unlock of the fenced path, a hand prune, and wrkslots recover. The locked registration must be discovered before anything irreversible: enumerate every repository registration of the checkout before the subvolume delete; unlock only wrkslots own lock (reason exactly the one wrkslots writes) and proceed; any foreign lock refuses up front with nothing changed. The post-delete failure path must leave state that create auto-recovery or recover finishes without hand git commands, and the rollback message must not claim a rollback it cannot do.

# Design

Pre-check in the subvolume-delete-first removal path (and any other removal path that can hit the same late discovery): before the bind unmount and subvolume delete, enumerate all repository registrations of each checkout being removed, not just the slots own repository. A registration locked with wrkslots own image-lock reason is unlocked and removal proceeds; any other lock refuses before any mutation. After the delete, a still-registered checkout must be left in a journal state recover can finish (unlocking only an own lock, pruning, archiving) instead of requiring hand git commands. Keep the change to the registration pre-check and the failure path; nearby seal-liveness work is a separate lane. Both editions if the Rust side has this path.

# Acceptance Criteria

Deterministic tests: (a) checkout registered in a second repo with wrkslots own lock: remove succeeds and leaves no registration; (b) foreign lock: refused up front, nothing deleted; (c) injected failure after the delete: recover or create auto-recovery finishes without manual git commands. Tests fail before the fix and pass after; condition-based waits only.
