---
title: 'wrkslots-image-mount-family-handlers: dispatch-shaped Rust handlers for image mount set'
status: open
priority: 2
issue_type: task
created_at: 2026-11-09T04:17:31.000000000+00:00
updated_at: 2026-11-09T04:17:31.000000000+00:00
---

# Description

Following #1095 wrkslots-image-status-handler, the image mount
family needs dispatch-shaped handlers in `imagecmd.rs`:
`image set-default`, `image trim`, `image grow`, `image mount`,
and `image unmount`. The pure cores and argv plans are landed
(`image_record`, `image_mounts`, `image_ops`); each handler returns
`(exit, stdout, stderr)` byte-identical to the Python edition on
post-load inputs, through the `PrivilegedRunner` seam, so tests
never mount or touch a live image.

# Design

- Order inside the issue: set-default (pure config decision),
  trim and grow (linear plans already in `image_ops`), then
  mount/unmount (the ensure-mounted state machine, including the
  discarding-stale-copy phase and residual loop holders).
- Each step lands unwired with an in-crate oracle differential in
  the #1095 style; the CLI-level differential lands with the
  dispatch wiring, as recorded for image status.
- No shared file is edited; dispatch arms are proposed in each
  landing report.

# Acceptance Criteria

- Oracle differentials pass for every handler landed under this
  issue, including refusals (not image-backed, shrink, residual
  holders, discarding phase).
- Handlers land unwired, gate green after the 04:00 ET blackout,
  arms sent to the coordinator.
- Tests use fixture registries, temp images, and fake runners only.

# Notes

Part of #562 rust-primary-wrkslots P8. The 00:15-04:00 ET gate
blackout applies: development first, gate after it ends.

Dispatch wiring progress (mu-rswrk2, 2026-11-12): image status
(#1095), sandbox show-config (#1101), limits show (#1112), and
image set-default are wired in wrkslotsd with CLI differentials
and landed (b3a57d10, 7edf3bf2, 048c94e2, b114b01a). Remaining:
trim/grow/mount/unmount, then the worktrees-image family (#1141)
and migrate/convert (#1291 signatures).

Privileged-path resolution for the mount family (coordinator
decision, 2026-11-12): no PATH lookups and no guessing. The
python3 used to run imagehelper.py is the interpreter the editions
launcher already runs the Python edition with, passed down as an
absolute path (add one launcher env var for it if none is
exported). imagehelper.py resolves through that interpreter's
installed wrkslots package (the module file beside slotimage.py),
and before running it the glue checks it is a regular file owned
by the user and not group- or world-writable, refusing with a
clear message otherwise. An env override is allowed only as an
absolute path, only for tests or an explicit operator choice, with
the same check applied.
