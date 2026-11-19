---
title: 'wrkslots-eden-multiple-backing-repos: accept multiple in-image Eden backing repos'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
created_at: 2026-11-17T15:30:00-05:00
updated_at: 2026-11-17T15:30:00-05:00
---

# Description

[watermelon-full-preview] `worktrees_image.eden_backing_repo` takes one name; accept a list or map (each under `.eden-backing/`), with `create --eden NAME=<name>` choosing one; `seed-eden-backing` makes a LAZY `sl clone -U <paths.default>` instead of a full local clone and sets `[remotefilelog] cachepath` to an in-image hgcache.

# Design

- Design sent to and approved by the coordinator before coding, with adjustments: strict per-entry validation (unique names; a relative, normalized suffix without `..`, rechecked inside `.eden-backing/` after resolving symlinks); `create --eden NAME=<name>` matches a configured name or its resolved path and refuses listing the configured entries; seeding lazily clones the source's remote `[paths] default` (refusing a missing or local one), writes `[remotefilelog] cachepath` only into the new clone's own hgrc, and proves standalone readability. Config widening only: no persisted field changes. The list/map forms need every wrkslots reader on the host to include this change: a pinned older reader refuses a non-string `eden_backing_repo` (the same caveat as a newly added key).

# Acceptance Criteria

- Coordinator approves the design; then list/map config, named create selection, and lazy seeding land with tests.

# Notes
