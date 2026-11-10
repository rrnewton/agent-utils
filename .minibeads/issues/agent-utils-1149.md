---
title: 'worktrees-image-migrate-original-kept: migrate leaves the original under the bind, nothing reclaims it, and a later remove breaks'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-09T17:35:00.000000000+00:00
updated_at: 2026-11-09T17:35:00.000000000+00:00
---

# Description

Found by the first real adopter of the worktrees image (a consumer harness on the shared gate host), by reading agent-utils cc487cc3 (`py/wrkslots/worktreesimage.py`, `imagecmd.py`, `cli.py`). Line numbers are at that commit. The project image representation (`worktrees_image`) is one sparse btrfs file mounted at `<control>/.slots-image`, with one subvolume per slot bind-mounted at `worktrees/slots/<name>`. "Original" means the slot directory on the host file system that `wrkslots migrate` copies from.

`imagecmd.py:786 _cmd_migrate` copies, compares, then `bind_slot`s over the original and prints "original kept
beneath". No wrkslots command ever deletes that original. Consequences:
1. Space: every migrated slot keeps twice its size on the host.
2. Reboot window: until the bind is re-established, the slot path shows the stale original, and an agent started
   there silently diverges from the real copy.
3. Removal (by code reading): `cli.py:4668 _remove_slot_directory` runs `unbind_slot`, `delete_slot_subvolume`,
   then `path.rmdir()`. With the original underneath, rmdir fails with ENOTEMPTY AFTER the record was dropped and
   the subvolume (the real slot) was deleted, and the stale original is left exposed. `relocate_bind` (used by the
   removal fence) renames the original along with the empty mount point, so the stale tree follows the fence.

# Design

Either discard the original as part of migrate, after the compare passes and while the same idle checks hold, or
add `worktrees-image reclaim-original <slot>`, which reaches the original through a private non-recursive bind of
the parent and deletes it under the #1150 census. In both cases `_remove_slot_directory` must refuse BEFORE
`delete_slot_subvolume` when anything is underneath the bind. Also strengthen migrate's compare (today it checks
only path, kind and size): compare content or hashes, mode, HEAD and Git status, and preserve hard links.

# Acceptance Criteria

- After migrate (or reclaim), nothing remains under the bind, and the space is freed.
- remove with something under the bind refuses before deleting the subvolume (test).
- migrate's compare catches a content-only difference (test).

# Notes

Order of the worktrees-image set: #1147 (records race), #1148 (silent mount failure), #1149 (migrate original), then #1150 (migrate liveness) and #1151 (help and small items). Rust parity follows in the Rust worktrees-image handlers.

## Resolution

Covered by f7477253 (#1162): `migrate-layout --to direct` already reclaims the host original beneath each bind under the layout census, and direct-layout migrate reclaims its plain original; those parts needed no new work.

Delivered here for the existing host-to-image `wrkslots migrate`: (1) binds-layout migrate now reclaims the original at migrate time — after the compare passes it renames the verified original to a `.<slot>.premigrate-<random>` sibling, binds the subvolume at the slot path, re-proves the displaced tree unused with the one-slot census (holds, registered liveness, process-use census), and deletes it, so nothing remains under the bind; a crash between rename and bind leaves the original at the named sibling to rename back. (2) `_remove_slot_directory` unmounts without dropping the record and refuses before `delete_slot_subvolume` when anything sits beneath the bind, re-establishing the bind with record and subvolume intact (test). (3) The migrate compare now covers mode, symlink target and a SHA-256 content hash per file, plus each checkout's HEAD and `git status` (read with `--no-optional-locks` so a copied linked worktree never writes the shared index); a same-size content-only difference refuses with the original kept (tests). Hard-link preservation in the copy was split out by coordinator decision and is not filed yet. Rust parity: mirror the signature fields, Git-state compare, reclaim order, and the remove guard in the Rust worktrees-image handlers (filed with the landing PUSH).
