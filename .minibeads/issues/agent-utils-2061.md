---
title: 'wrkslots-image-status-mounts: image status must be read-only'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T11:45:00-05:00
updated_at: 2026-11-21T11:45:00-05:00
---

# Description

`wrkslots image status` re-mounted slot images it had just unmounted (new loop devices), so a status command has a mounting side effect and leaks mounts.

# Design

[watermelon-full-preview] Cause: `cli._load_config` runs `_activate_representation`, which mounts every unmounted image slot (`slotimage.ensure_all_mounted`) on every command, including the read-only `image status`. Fix: `_load_config(..., mount_images=False)` skips the ensure-mount (mount state is only observed via `_refresh_owned_mounts`); `image status` loads with it, all other commands keep the ensure-mount. The Rust edition's loader already never activates, so this also closes that divergence. No persisted-format or lock change.

# Acceptance Criteria

- The behaviour described under Design, covered by a deterministic regression test (condition-based, no wall-clock budgets): `test_image_status_readonly.py` proves status on an unmounted slot never calls the ensure-mounted path and reports `mounted=False`, and that an ordinary load still ensures mounts.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
