---
title: 'wrkslots-worktrees-image-concurrent-bind-stacks: concurrent config loads stack identical binds on one checkout'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-24T03:15:00-05:00
updated_at: 2026-11-24T03:15:00-05:00
---

# Description

Every wrkslots config load calls worktreesimage.ensure_binds with no lock. It reads the mount table, and bind_slot then runs mount --bind with no re-check, so concurrent wrkslots commands after a boot each see a checkout unmounted and each bind it. After a host reboot, most bound checkouts in one registry were found with two identical binds of the same subvolume stacked at one path, and reap refuses on its stacked-bind check.

# Design

Re-check and bind under a lock, keeping the work under the lock constant-time per slot (no scans of the whole registry). Collapse identical stacked binds of the same subvolume, leaving one, and update the bind record's mount_id to the bind that stays; never touch a stack whose layers differ. Tests use fakes or the existing test harness only; nothing real is mounted or unmounted.

# Acceptance Criteria

- A test that races two config loads asserts exactly one bind is created.
- Identical stacked binds collapse to one with the surviving mount_id recorded; a stack whose layers differ is left untouched.
- No whole-registry scan runs under the bind lock.

# Notes

Field evidence is from a consumer registry after a host reboot (24 of 25 bound checkouts double-bound). Root use on real images is out of scope for the fix's tests.

## Resolution

[watermelon-full-preview] Landed on github/main as adf128fa. Config loads now establish each bind under the bind-records flock (reentrant for its holder, bounded at 120s, double-checked so the common all-mounted load never takes it), and ensure_image_mounted holds the same lock across its check and mount, so racing loads bind exactly once per checkout (proven by a two-thread race test that binds twice on the old code). Stacked binds left by older code are never unmounted on load: they surface read-only in worktrees-image status (doubled_binds) and are collapsed only by the explicit 'wrkslots worktrees-image collapse-binds' command (dry run by default; --apply re-reads the mount table before every umount, requires identical device/root/source/fstype/options/propagation in unambiguous mount-id order, unmounts only the top copy, skips and reports a busy layer as busy, and exits 3 when anything was skipped). Python only: the Rust daemon has no config-load bind path, so no wt_mount mirror was needed. Operators: collapse a doubled slot before removing or relocating it.
