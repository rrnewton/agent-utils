---
title: 'worktrees-image-help-stale: worktrees-image help describes the old mount-at-slots design, plus small hardening items'
status: closed
priority: 3
issue_type: task
created_at: 2026-11-09T17:35:00.000000000+00:00
updated_at: 2026-11-09T17:35:00.000000000+00:00
---

# Description

Found by the first real adopter of the worktrees image (a consumer harness on the shared gate host), by reading agent-utils cc487cc3 (`py/wrkslots/worktreesimage.py`, `imagecmd.py`, `cli.py`). Line numbers are at that commit. The project image representation (`worktrees_image`) is one sparse btrfs file mounted at `<control>/.slots-image`, with one subvolume per slot bind-mounted at `worktrees/slots/<name>`. "Original" means the slot directory on the host file system that `wrkslots migrate` copies from.

- `worktrees-image --help`, `worktrees-image mount --help` and the module docstring say the image is "mounted at
  the slots directory". The code mounts it at `<control>/.slots-image` and bind-mounts each slot.
  `mount_image`/`unmount_image` (`worktreesimage.py:247, 273`) still implement the old design and look unused.
- `create_image` creates the image file 0644, so any local user can read every slot through it. Per-slot images
  use 0600.
- `worktrees-image create` reads only the live config. Enabling the config before the image exists makes every
  `create` refuse (validate checkouts included). Offer `worktrees-image create --path P --size-bytes N` before
  enabling, or document the order.

# Acceptance Criteria

- Help and docstrings match the bind design; dead old-design helpers removed or justified.
- The image file is created 0600 (test).
- The enable order is either unnecessary or documented in help and the USER_GUIDE.

# Notes

Order of the worktrees-image set: #1147 (records race), #1148 (silent mount failure), #1149 (migrate original), then #1150 (migrate liveness) and #1151 (help and small items). Rust parity follows in the Rust worktrees-image handlers.

## Resolution

Most stale help and docstring text was already corrected by f7477253 (#1162), which revived `mount_image`/`unmount_image` for the direct layout — they are kept, now documented as mounting at the given mount point. Remaining items delivered here: the module safety rule now names the configured mount point (not always the slots directory); `create_image` creates the backing file 0600 like the per-slot images, since it holds every slot (test); and `wrkslots worktrees-image create --path <file> --size-bytes <n>` creates the image without reading a `worktrees_image` section (same validation as a configured pair; the two flags must come together), so the file can exist before the section is enabled. The USER_GUIDE documents the enabling order (create and enable in either order, mount before any slot-writing command). Rust parity: mirror the 0600 creation mode, the `--path`/`--size-bytes` create form, and the doc wording (filed with the landing PUSH).
