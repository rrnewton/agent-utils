---
title: 'wrkslots-worktrees-image-handlers: dispatch-shaped Rust handlers for worktrees-image'
status: open
priority: 2
issue_type: task
created_at: 2026-11-09T15:42:10.000000000+00:00
updated_at: 2026-11-09T15:42:10.000000000+00:00
---

# Description

Following #1116 wrkslots-image-mount-family-handlers, the
`worktrees-image` family (status, create, mount, unmount, grow,
trim) needs dispatch-shaped handlers returning
`(exit, stdout, stderr)` byte-identical to the Python edition.
The pure core and argv plans are landed in `worktrees_image.rs`
(settings, classify_mount, listing_signature, bind records, ops
plans behind the PrivilegedRunner seam).

# Design

- Handlers added to the already-registered `worktrees_image`
  module; no new lib.rs line and no shared file edited.
- Every privileged effect (mkfs.btrfs, loop mount, bind mounts,
  btrfs quota/resize, fstrim) goes through the fake-runner seam in
  tests; nothing mounts a real image.
- Oracle differentials in the #1095/#1112 in-crate style drive the
  Python handlers on shared fixtures and compare bytes including
  the unconfigured refusal.
- Dispatch arms are proposed in the landing report; the CLI-level
  differential lands with the wiring.

# Acceptance Criteria

- Oracle differentials pass for status (human/json), create,
  mount, unmount, grow and trim plans and outputs.
- Handler step lands unwired, full gate green.
- Fixtures and temp images only.

# Notes

Part of #562 rust-primary-wrkslots P8. `migrate` follows under
its own step; run/box after that.
