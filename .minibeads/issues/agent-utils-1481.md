---
title: 'wrkslots-salvage-kept-commit-missing-objects: a dangling kept commit whose objects are missing locally parks a slot forever'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
- salvage
created_at: 2026-11-12T18:00:00.000000000+00:00
updated_at: 2026-11-12T18:00:00.000000000+00:00
---

# Description

[watermelon-full-preview] A consumer's batch removal refused: "salvage of checkout <nested repo> cannot bundle kept commit 19c0f4a5...: object f1e48e3d... is missing from every object store searched; nothing was removed". 19c0f4a5 is a parentless deploy commit and f1e48e3d is its own tree; the store has no alternates and is not a partial clone, so the content is unrecoverable locally. No ref, reflog, or packed-ref contains the commit (dangling), and it is in none of the 750 rescue refs pushed for that slot. One 25 GiB slot is parked on it; more from the same era may hit it.

# Design

Treat a kept commit whose own tree/objects are missing locally (and not promisor-backed) like the #1278 promisor-gap exclusion: exclude it from the bundle, record it in the salvage receipt with the missing object ids and the reason ("objects missing locally; content unrecoverable; not on any remote"), and continue. Refuse only if the commit is reachable from a ref or reflog the slot would otherwise lose (real loss evidence). Rust parity if the Rust salvage path exists.

# Acceptance Criteria

- Fixture with a dangling commit whose tree object is deleted: salvage proceeds, with the receipt entry naming the commit, missing object, and reason.
- A ref-reachable (or reflog-reachable) commit with a missing object still refuses, naming commit, object, and stores searched.
- Existing promisor-gap exclusion and its receipt reason are unchanged.

# Notes

Reported 2026-11-12 from a consumer's batch removal. Extends #1278 wrkslots-salvage-bundle-object-store and #1280 wrkslots-kept-set-over-collection.

## Resolution

[watermelon-full-preview] Landed as 1748e7e5 on github/main: dangling kept commits whose objects no searched store holds are excluded and recorded (objects missing locally; content unrecoverable; not on any remote); ref- or reflog-reachable kept commits still refuse. Rust salvage_gate writes no bundles and keeps the same empty receipt field.
