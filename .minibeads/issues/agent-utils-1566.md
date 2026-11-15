---
title: 'eden-cap-outer-only: count only outer checkouts against the host Eden cap'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
created_at: 2026-11-15T14:10:00+00:00
updated_at: 2026-11-15T14:10:00+00:00
---

# Description

[watermelon-full-preview] `configuration.eden_host_cap` today counts every
Eden checkout on the host (`eden.count_checkouts` over every daemon config
dir), including checkouts inside the worktrees image. The owner wants the
cap to apply only to OUTER checkouts on the host filesystem: checkouts
inside the image are bounded by the image's disk, so Eden metadata
exhaustion stays isolated there (#1204 wrkslots-eden-checkouts).

# Design

- Count by path containment: a mount is in-image when its resolved path
  (`os.path.realpath`, applied to mount and roots; alias spellings of one
  checkout dedupe to one) lies under the image mount point, the image
  mount directory, the validation-slots surface, or a recorded slot bind
  target. Bind mounts are invisible to path resolution, so recorded bind
  targets are explicit roots; image content bound to an unrecorded host
  path still counts outer (conservative). Foreign content bound into a
  root would under-count; mount-table hardening is out of scope.
- Fail-safe: no worktrees image, an unmounted image, or unreadable bind
  records give no roots, so every checkout counts outer as before.
- `configuration.eden_host_cap` is reinterpreted as the OUTER cap (no key
  rename); hosts configured with a total cap effectively gain unbounded
  in-image checkouts, which is the requested behaviour. The create
  refusal names both sets. Status and doctor report both counts
  (nested `eden` JSON object plus a text line, only for projects with
  Eden rows), and doctor adds an `eden-cap` finding at the outer cap.

# Acceptance Criteria

- Stubbed-runner tests: an in-image checkout is not counted, an outer one
  is, and the cap refuses at outer N with in-image checkouts present.
- `test_eden.py` green; capped `make validate` and the gate green.

# Notes

Resolution: (fill in at landing)
