---
title: 'worktrees-image-mount-failure-silent: a failed image mount at config load is swallowed, and restore revives slot agents into bare paths'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-09T17:35:00.000000000+00:00
updated_at: 2026-11-09T17:35:00.000000000+00:00
---

# Description

Found by the first real adopter of the worktrees image (a consumer harness on the shared gate host), by reading agent-utils cc487cc3 (`py/wrkslots/worktreesimage.py`, `imagecmd.py`, `cli.py`). Line numbers are at that commit. The project image representation (`worktrees_image`) is one sparse btrfs file mounted at `<control>/.slots-image`, with one subvolume per slot bind-mounted at `worktrees/slots/<name>`. "Original" means the slot directory on the host file system that `wrkslots migrate` copies from.

- `cli.py:4303-4321`: on every config load, `ensure_image_mounted` and `ensure_binds` run inside
  `except worktreesimage.WorktreesImageError: pass`, with no warning.
- `agentctl restore` (`rs/agentctl/src/restore.rs` `remount_slot_images`) runs `wrkslots image status`, which
  triggers that load, but it only reads the per-slot ext4 warnings ("slot image not mounted"). A worktrees image
  that failed to mount is invisible to it, so restore revives every slot agent into empty mount-point directories,
  or into stale originals while #1149 stands.
- No mount unit exists; mounting happens only as a side effect of the first wrkslots command.

# Design

(a) Config load prints a parseable warning, e.g. `wrkslots: worktrees image not mounted: <reason>`, matching the
per-slot path, and `image status` reports it.
(b) restore treats that warning, or a failed mount unit, as fatal FOR SLOT AGENTS only: it still revives agents not
in a slot, skips every agent whose cwd lies in a slot whose recorded bind is missing, and records each skip with a
reason. (This restore half is tracked with the slot-rebind work on the restore side.)
(c) A mount unit ordered before restore, shipped by `agentctl install-units` or wrkslots.

# Acceptance Criteria

- A failed mount produces the warning (test with a fake mount failure).
- restore refuses to revive a slot agent whose bind is missing and says why; agents not in a slot still revive.
- The mount-unit story is documented in the USER_GUIDE.

# Notes

Order of the worktrees-image set: #1147 (records race), #1148 (silent mount failure), #1149 (migrate original), then #1150 (migrate liveness) and #1151 (help and small items). Rust parity follows in the Rust worktrees-image handlers.
