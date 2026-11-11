---
title: 'wrkslots-image-status-worktrees-field: conditional worktrees image in image status'
status: open
priority: 2
issue_type: task
created_at: 2026-11-11T10:00:00.000000000+00:00
updated_at: 2026-11-11T10:00:00.000000000+00:00
---

# Description

Python #1148 (24190b53): `wrkslots image status` gains a
worktrees-image line and a JSON `worktrees_image` field, only when
the section applies on this host; unconfigured and skipped output
stays byte-identical. Config load also warns once per process on
stderr when the worktrees image cannot be mounted at load
(`wrkslots: WARNING: worktrees image not mounted: <reason>`),
while load still succeeds and writers refuse on state.

# Design

- In `imagecmd.rs`: `image_status` takes an optional injected
  worktrees-image report (built from the landed
  `worktrees_image::status_report` core) and renders the human
  line after `new slots:` and the JSON field in the Python
  payload's insertion order.
- In the config path the shared core owns: compute the injected
  state once at load and print the warning once per project root
  per process when the applied image is not mounted.
- Oracle differentials cover unconfigured (byte-identical),
  skipped (byte-identical), applied unmounted (line, field, and
  warning), and applied with injected mount facts.

# Acceptance Criteria

- Image status human and JSON match Python on all four cases.
- The warning appears exactly once per process and only on
  stderr.
- Handler lands unwired with focused tests green; full gate per
  the lane's usual pattern.

# Notes

Part of #562 rust-primary-wrkslots P8. #1149 follows.
