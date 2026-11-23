---
title: 'wrkslots-post-provision-submodule-hook-outside-worktree: a post-provision submodule hook runs where git sees no working tree'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-23T00:45:00-05:00
updated_at: 2026-11-23T00:45:00-05:00
---

# Description

Creating a slot for one configured project fails in post-provision hook 1: `git submodule update --init --recursive` refuses with "git-submodule cannot be used without a working tree". `wrkslots recover` re-runs the hook and fails the same way. Slots of another project on the same registry are created normally, so the difference is in that project's slot layout or hook configuration (for example a bare or separate-git-dir checkout, a GIT_DIR or core.bare setting inherited by the hook, or the hook running with a cwd outside the new worktree).

# Design

- Reproduce with a scratch registry whose project has submodules and the affected layout.
- Run post-provision hooks with cwd set to the slot worktree and with GIT_DIR, GIT_WORK_TREE and similar variables cleared or set explicitly for that worktree.
- If the layout itself has no working tree, refuse at config validation with a clear message instead of failing in the hook.

# Acceptance Criteria

- A test creates a slot of the affected layout with a submodule hook and the hook succeeds; recover after an injected hook failure succeeds once the cause is fixed.

# Notes

Reported by a consuming team on a shared host; filed by the coordinator. Relevant to the wrkslots V2 design (#2138).
