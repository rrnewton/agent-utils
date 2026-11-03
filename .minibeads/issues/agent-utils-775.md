---
title: 'wrkslots-stray-worktree-reap: reap stray git worktree registrations nobody owns'
status: closed
priority: 2
issue_type: task
assignee: mu-rust@devbig030
labels:
- wrkslots
created_at: 2026-11-02T16:58:21.496057941+00:00
updated_at: 2026-11-03T03:07:10.620194604+00:00
closed_at: 2026-11-03T03:07:10.620194464+00:00
---

# Description

[opus 5.5] Owner goal (2026-11-02): exactly one slot per live agent plus one per running validate, and ownerless slots past their TTL retired deterministically. From a consumer team's wrkslots cleanup audit; its B1 and B2 were fixed in 2762da0f. Stray 'git worktree' registrations accumulate and are never reaped (97 under one lane's ignored/ directory in a consumer repo). wrkslots should find registrations that belong to no slot and no live process, report them, and prune them deterministically (dry-run first, refuse anything with a live process or unpushed work, preserve a rescue ref).
