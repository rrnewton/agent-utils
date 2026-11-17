---
title: 'wrkslots-registration-repo-probe-walks-up: a non-git directory inside another git tree is not a repository'
status: closed
priority: 1
issue_type: bug
assignee: mu-lockedreg
labels:
- wrkslots
created_at: 2026-11-17T16:24:28.331876753+00:00
updated_at: 2026-11-17T17:07:35.326725976+00:00
closed_at: 2026-11-17T17:07:35.326725856+00:00
---

# Description

Follow-up to #1766. _registration_repositories in py/wrkslots/cli.py recognises a repository with git rev-parse --git-dir run inside the candidate directory. In a non-git directory that sits inside another git tree that succeeds by walking upward, so the directory is classified as a repository and the parent repository would be pruned and listed during a validate removal. Recognition must compare rev-parse --show-toplevel (or an absolute --git-common-dir) against the candidate path itself. The same function also branches on getattr(probe, 'worktree_lock_state') to accommodate a test double; replace that with an explicit parameter.

# Design

Probe with git rev-parse --show-toplevel and require the resolved top-level to equal the candidate directory; skip anything else. Thread an explicit probe parameter instead of the getattr test-double check; test doubles pass it explicitly.

# Acceptance Criteria

A non-git directory inside a git tree is not returned as a registering repository, and the parent repository is not pruned or listed for it; real repositories (including the recorded one) still enumerate. Deterministic tests fail before and pass after.

# Notes

## Resolution

[watermelon-full-preview] Landed on main as `7c3e86e2004e174aa6407affa6ae8a58f9a8016d` (gate pass on `8feb734d`). `_is_own_git_repository` recognises a bare repository when `--is-bare-repository` is true and `--absolute-git-dir` resolves to the candidate itself, and any other repository when `--show-toplevel` resolves to the candidate; a non-git directory inside a work tree or inside a bare repository is rejected, so its parent is never pruned or listed for it. The `getattr` test-double branch from #1766 is replaced with the explicit `probe_git` parameter, threaded through `_validate_subvolume_delete_first` as `registration_probe`. Tests cover the bare, inside-bare, inside-worktree, and real-repository cases.
