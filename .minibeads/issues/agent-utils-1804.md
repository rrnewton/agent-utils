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

- Design to the coordinator before coding (this issue tracks that gate).

# Acceptance Criteria

- Coordinator approves the design; then list/map config, named create selection, and lazy seeding land with tests.

# Notes
