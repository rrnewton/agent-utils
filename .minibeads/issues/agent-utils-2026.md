---
title: 'wrkslots-fence-breaks-nested-submodule-gitdir: fencing a slot breaks salvage in nested worktrees whose submodules record the unfenced path'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T05:30:00-05:00
updated_at: 2026-11-21T05:30:00-05:00
---

# Description

[watermelon-full-preview] Field report: fencing a slot (renaming it to `.NAME.fenced.N.ID`) breaks salvage in nested worktrees. A nested linked worktree inside the slot holds a submodule whose admin `core.worktree` is a relative path naming the slot's unfenced location (resolved relative to the submodule's gitdir, which lives outside the slot). After the rename Git cannot chdir into the submodule: `fatal: cannot chdir to '../../.../worktrees/slots/<slot>/ignored/<nested>/agent-utils/<sub>': No such file or directory`, then `fatal: 'git status --porcelain=2' failed in submodule <sub>`. Salvage's worktree index read (`_index_state_trees`, `git add -u`) dies on that, and `recover` of the interrupted removal refuses with rc 3, so the slot cannot be removed until a hand repair.

Status already survives this shape (#1483: per-invocation submodule overrides, no Git file rewritten); the staged/worktree index read does not.

# Design

- In `_index_state_trees` (`py/wrkslots/cli.py`), when `git add -u` fails with the stale-submodule signature (`cannot chdir to`, `failed in submodule`), fall back per-invocation, mirroring the status read: take every populated direct gitlink out of the *copied* index, add the rest of the worktree, then put each gitlink back at the submodule's current HEAD, read straight from its gitdir (`git --git-dir=<gitdir> rev-parse`), which needs no worktree at all.
- Only the copied index is written. No Git file, journal, registry field, or lock file changes, so a refused or rolled-back removal leaves repository metadata exactly as it was, and no reader/writer pairing is affected.
- The salvage index path is Python-only; the Rust edition has no equivalent salvage index read.

# Acceptance Criteria

- A deterministic regression test: a nested linked worktree with a submodule inside a slot, the slot directory renamed to its fenced name, and salvage index trees still read, with the worktree tree carrying the submodule's gitlink at its current HEAD.
- The existing nested-worktree repair tests keep passing, including the refusal for a live pinned `core.worktree` that names another live worktree.
- No persisted state changes: no new journal, seal, registry, or lock-file fields or files.

# Notes

Related: #1483 (status read of the same stale-`core.worktree` shape), #788 (nested repository repair after a slot move), #1735 wrkslots-remove-holds-state-lock-through-salvage (the fence this rename belongs to).
