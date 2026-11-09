---
title: 'wrkslots-migrate-convert-handlers: dispatch-shaped Rust handlers for migrate and image convert'
status: open
priority: 2
issue_type: task
created_at: 2026-11-09T20:27:19.000000000+00:00
updated_at: 2026-11-09T20:27:19.000000000+00:00
---

# Description

Following #1141 wrkslots-worktrees-image-handlers, `wrkslots
migrate` and `wrkslots image convert` need dispatch-shaped Rust
handlers returning `(exit, stdout, stderr)` byte-identical to the
Python edition. Migrate copies one idle slot into a btrfs
subvolume in the mounted worktrees image, verifies the copy file
by file, and binds it at the same path with the original kept
beneath. Convert swaps one slot between worktree and per-slot
image representations through a staging copy, restoring the
original on failure.

# Design

- Handlers in the already-registered `worktrees_image` and
  `imagecmd`/`image_ops` modules; no shared file edited.
- Copying is a real filesystem operation on fixture trees only;
  privileged subvolume, bind, provision, relocate and destroy
  steps go through the PrivilegedRunner seam and injected facts,
  and registry, lock, slot-used and Git-verification steps stay
  with the dispatch glue, as they do for the Python handlers'
  callers.
- In-crate oracle differentials in the #1095/#1112 style compare
  bytes for the refusal and no-op paths; success paths are proven
  with fixture trees and fake runners.
- Dispatch arms are proposed in the landing report; CLI-level
  differentials land with the wiring.

# Acceptance Criteria

- Handlers land unwired with tests green: migrate refusals and
  already-bound no-op, convert already-target no-op and the
  staging/copy/rename plan order in both directions.
- Full gate waits for the coordinator all-clear after 4 PM ET on
  2026-11-09.
- No real subvolume, mount, or live slot is touched in tests.

# Notes

Part of #562 rust-primary-wrkslots P8. Run/box handlers follow.
