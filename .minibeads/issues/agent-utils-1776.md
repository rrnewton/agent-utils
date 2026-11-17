---
title: 'wrkslots-registration-repo-probe-walks-up: a non-git directory inside another git tree is not a repository'
status: open
priority: 1
issue_type: bug
assignee: mu-lockedreg
labels:
- wrkslots
created_at: 2026-11-17T16:24:28.331876753+00:00
updated_at: 2026-11-17T16:24:28.331876753+00:00
---

# Description

Follow-up to #1766. _registration_repositories in py/wrkslots/cli.py recognises a repository with git rev-parse --git-dir run inside the candidate directory. In a non-git directory that sits inside another git tree that succeeds by walking upward, so the directory is classified as a repository and the parent repository would be pruned and listed during a validate removal. Recognition must compare rev-parse --show-toplevel (or an absolute --git-common-dir) against the candidate path itself. The same function also branches on getattr(probe, 'worktree_lock_state') to accommodate a test double; replace that with an explicit parameter.

# Design

Probe with git rev-parse --show-toplevel and require the resolved top-level to equal the candidate directory; skip anything else. Thread an explicit probe parameter instead of the getattr test-double check; test doubles pass it explicitly.

# Acceptance Criteria

A non-git directory inside a git tree is not returned as a registering repository, and the parent repository is not pruned or listed for it; real repositories (including the recorded one) still enumerate. Deterministic tests fail before and pass after.
