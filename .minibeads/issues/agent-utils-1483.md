---
title: 'wrkslots-fence-rename-breaks-linked-worktrees: remove renames the slot before salvage finishes its git reads, so a linked worktree submodule status fails and removal refuses mid-way'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
- salvage
created_at: 2026-11-13T00:00:00.000000000+00:00
updated_at: 2026-11-13T00:00:00.000000000+00:00
---

# Description

[watermelon-full-preview] `wrkslots remove` renames the slot to its fenced name (`<slots>/.<slot>.fenced.<n>.<id>`) before salvage finishes its git reads. A checkout in the slot has a linked worktree inside the slot (`<slot>/temp/<wt>`, gitdir under the main repository's `.git/worktrees/<slot>/modules/<repo>/worktrees/<wt>`) whose submodule's gitfile and `core.worktree` are absolute paths at the original slot path. After the rename, the post-fence salvage recheck runs `git status --porcelain=v2 ...` in the linked worktree and git fails with `fatal: cannot chdir to '<original slot>/temp/<wt>/<sub>': No such file or directory` and `fatal: 'git status --porcelain=2' failed in submodule <sub>`, so removal refuses mid-way (rc 3). At the original path `git status` works and shows ` M <sub>`. Dead slots of 20+ GiB each are parked on this refusal.

# Design

Decide and implement one of: gather salvage facts before the fence rename (the lock-free pregather from #1367 wrkslots-salvage-thin-bundle-fast may already allow it); run salvage's git reads with path translation (`GIT_DIR`/`GIT_WORK_TREE` overrides or `core.worktree` repair in a temp copy); or treat a submodule status failure inside a nested linked worktree as drift to archive rather than a refusal. Never lose work: if the submodule has local changes, they must be archived (`--archive-drifted-checkouts` semantics). Check Rust parity if the Rust edition has the same path.

# Acceptance Criteria

- A scratch slot whose checkout has a linked worktree with a submodule (absolute `core.worktree` at the original slot path) removes successfully after the fence rename, with the submodule's local changes archived, not lost.
- A regression test fails before the fix and passes after, with no wall-clock waits.
- Existing lifecycle, nested-worktree repair, and differential suites still pass.

# Notes

Reported 2026-11-12 from a consuming project's dead slots. Shape B (2026-11-13): a nested checkout under `ignored/` with a submodule hits the same refusal; both shapes are at their normal path with no journal left. Related: #1367 wrkslots-salvage-thin-bundle-fast (lock-free pregather), #1481 wrkslots-salvage-kept-commit-missing-objects (pending, same code).

## Resolution

[watermelon-full-preview] Landed in a24582b3. `_GitVcs.status` now falls back to per-invocation submodule overrides when a fence rename leaves a submodule's recorded `core.worktree` stale: the superproject is read without recursing, each initialized gitlink is read at its current path, and the exact `porcelain=v2` lines Git would have printed are synthesized (verified byte-identical). No on-disk Git file is rewritten, so a refused or aborted removal leaves shared repository metadata untouched (asserted in tests). Covers linked worktrees under `temp/` and nested checkouts under `ignored/`. Rust defers submodule salvage to Python, so no Rust change. Tests: `py/wrkslots/tests/test_fence_salvage_recovery.py`.
