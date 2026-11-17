---
title: 'wrkslots-validate-remove-subvolume-delete-first: delete a subvolume-backed validate slot subvolume-first'
status: closed
priority: 1
issue_type: task
labels:
- wrkslots
created_at: 2026-11-16T18:16:44-05:00
updated_at: 2026-11-16T19:28:10-05:00
closed_at: 2026-11-16T19:28:10-05:00
---

# Description

[watermelon-full-preview] Removing a subvolume-backed validate slot
takes 10-30 minutes, mostly the delete itself under the registry
lock. The census bound is only 540 s, so the slow part is not the
proof: `_finish_remove_paths` first empties the tree file by file
(the `_remove_cache_directory` unlink walk,
`_delete_root_owned_within_slot`'s walk, and `git worktree remove
--force` unlinking every file). Only then does
`_remove_fenced_directory` -> `_remove_slot_directory` unmount and
run `btrfs subvolume delete` on an already-empty subvolume.

# Design

- For subvolume-backed validate slots, after the unchanged proof
  (census and fence): unmount, check nothing sits underneath,
  `btrfs subvolume delete` the whole slot subvolume, then prune the
  Git worktree registration (`git worktree prune`, or remove the
  admin directory after checking it points into the slot). Skip the
  per-file walks.
- `btrfs subvolume delete` refuses nested subvolumes, so it cannot
  reach outside the slot; such a refusal keeps the slot.
- Keep the old path for non-subvolume layouts, unchanged.
- Coordinate with the private-finish lock work: whichever lands
  first, the other rebases.

# Acceptance Criteria

- A stub-runner test records that no per-file walk happens for a
  subvolume-backed validate slot and that the Git registration is
  pruned and the branch handling matches the old path.
- Non-subvolume validate slots and agent slots remove exactly as
  before.
- A nested-subvolume or underneath-content refusal deletes nothing
  and keeps the slot recoverable.

# Notes

## Resolution

[watermelon-full-preview] Landed on main as
`86305bb23089db1ad6862782ed193315e423ed40` (gated `bc63defc`,
owner-priority gate pass). `_finish_remove_paths` now tries
`_validate_subvolume_delete_first` after the unchanged census/fence
proof and validation-proof recheck: for a worktrees-image-backed
validate slot it unmounts the bind, refuses when content sits
underneath (bind restored, nothing deleted), deletes the whole slot
subvolume, prunes each checkout registration with `git worktree
prune`, and deletes the branch, skipping the per-file walks.
Non-subvolume layouts, per-slot disk images, and slots with
initialized submodules keep the old path. Stub-runner tests record
the umount/delete/prune order with no per-file walk and the
underneath-content refusal deleting nothing; they fail on the parent
code and pass landed.
