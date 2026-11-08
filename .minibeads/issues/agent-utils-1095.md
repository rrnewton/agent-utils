---
title: 'wrkslots-image-status-handler: dispatch-shaped Rust handler for image status'
status: open
priority: 2
issue_type: task
created_at: 2026-11-08T22:55:45.000000000+00:00
updated_at: 2026-11-08T22:55:45.000000000+00:00
---

# Description

The P8 image core modules are landed (`image_record`, `image_mounts`,
`image_status`, `image_ops`) but no `wrkslots image` command runs in
Rust yet, because dispatch (`cli.rs`, soak, editions) belongs to the
shared-core lane. The first dispatch-shaped step is `image status`:
a handler in a new mu-rsimg-owned `imagecmd.rs` that the shared core
can wire with a small dispatch arm, proven byte-identical to the
Python edition on the same inputs before it is wired.

Python behaviour to match (`py/wrkslots/imagecmd.py:865`,
`slotimage.usage`): list `new_slot_representation` and, per image
record, slot/state entries with image path, ceiling and host
allocated bytes, mounted flag and backend, and (when mounted) used
and free bytes and used inodes; human or JSON rendering; an invalid
image record refuses with the oracle's text and exit 3.

# Design

- New `rs/wrkslotsd/src/imagecmd.rs` (mu-rsimg-owned): handler takes
  injected values (control directory, configured representation,
  output format, mount-table facts, statvfs facts) plus a
  `PrivilegedRunner` where an effect could ever be needed (status
  needs none), and returns `(exit, stdout, stderr)`.
- Reuse the landed cores: `image_record::all_images`,
  `image_mounts` classification, `image_status::usage_report` and
  the human/JSON renderers. No edits to `cli.rs`, dispatch, soak, or
  editions; the exact dispatch arm is proposed in the landing report
  for the shared-core lane to add.
- Tests use fixture registries and sparse temp images only; nothing
  is mounted and no live slot image is touched.
- A differential test compares the Rust handler's
  `(exit, stdout, stderr)` with the Python edition's on the same
  fixture inputs, human and JSON, mounted and unmounted, empty and
  invalid-record cases.

# Acceptance Criteria

- The differential test passes on human and JSON renderings,
  including the empty registry and the invalid-record refusal.
- The handler lands unwired (module plus differential), gate green,
  with the exact dispatch arm text sent to the coordinator.
- No shared file is edited by this lane.

# Notes

Part of #562 rust-primary-wrkslots P8. Handler-ready list and the
following handlers (sandbox show-config, then limits) are tracked in
the lane's reports to the coordinator.
