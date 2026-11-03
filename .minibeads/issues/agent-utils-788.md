---
title: 'wrkslots-remove-nested-submodule-worktree: removing a finished slot is refused when nested submodules hold an absolute core.worktree'
status: closed
priority: 1
issue_type: bug
assignee: mu-rust@devbig030
labels:
- wrkslots
created_at: 2026-11-02T19:03:14.047701199+00:00
updated_at: 2026-11-03T03:07:21.246729615+00:00
closed_at: 2026-11-03T03:07:21.246729505+00:00
---

# Description

[opus 5.5] Reported by a consumer team 2026-11-02; blocks a worktree trim the owner asked for on one host. 'wrkslots remove' of a finished slot (no unpushed work, owner gone) is REFUSED: removal first renames the slot to a fenced name, then 'git status' fails inside the nested submodules because their core.worktree is an ABSOLUTE path to the old location. Fix: check status before the rename, or rewrite/ignore an absolute core.worktree in nested submodules during the check (git worktree and submodule moves have the same problem). Note 1fa19589 already pins GIT_WORK_TREE for linked-worktree enumeration after a fence rename; confirm whether the reporter ran an older build and whether the status check path is still uncovered. Regression test: a slot with a populated nested submodule whose core.worktree is absolute; remove succeeds, nothing salvaged wrongly. The slot in question is intact; do not retry removals on it until fixed.
