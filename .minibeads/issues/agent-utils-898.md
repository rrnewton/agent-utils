---
title: 'wrkslots-image-remove-own-bind: remove a slot born in the worktrees image despite its own bind mount'
status: open
priority: 1
issue_type: bug
assignee: mu-validate
created_at: 2026-11-05T16:33:33.682649528+00:00
updated_at: 2026-11-05T16:33:33.682649528+00:00
---

# Description

In the worktrees-image mode, `wrkslots remove` cannot remove a slot that was created inside the image as a bind-mounted subvolume. The lead's probe slot hit this on 2026-11-05. Removal refuses with:

    live process 1 uses slot .../worktrees/slots/<slot>: mount=.../worktrees/slots/<slot>

The live-use scan walks /proc/1's mounts and counts the slot's own wrkslots-managed bind mount as an occupant. So no slot born in the image can ever be removed.

# Design

- `remove` (and any other live-use scan that guards a destructive step) recognises the bind mount wrkslots itself made for the slot, as recorded in the worktrees-image record: the source subvolume, the target path and the mount ID. That mount is not counted as an occupant. Any other mount of, or under, the slot still refuses, as now.
- Removal then unmounts the managed bind as one of its own steps, after the occupant checks pass and before deleting, and deletes the subvolume. Order and failure handling follow the existing remove protocol: a failed unmount refuses with the reason and leaves the slot and its record intact.
- Match the mount by its recorded identity, not just by path, so a foreign mount at the same path is still treated as an occupant.

# Acceptance Criteria

- A test runs a create-in-image then remove cycle: the slot, its bind mount and its subvolume are all gone afterwards, and the image record no longer lists the slot.
- A test where a second, foreign mount sits under the slot: remove still refuses, naming that mount.
- A test where unmounting fails: remove refuses and leaves a state that a retried remove can finish.

# Notes

Reported by the lead on 2026-11-05 at 11:33 AM ET (P2). The worktrees-image lane has finished, so this is queued for a wrkslots lane: mu-validate after #894 (also wrkslots remove).

Seen once, possibly unrelated: removing a validation probe slot refused with "validation-run authority is unverifiable ... cannot enumerate user-systemd state: Failed to get property ControlGroup ... Remote peer disconnected". It looks like a transient user-systemd bus error. If it recurs, consider retrying that read once before refusing.

Raised to P1 by the lead on 2026-11-05 at 12:38 PM ET. With the worktrees-image mode on in that harness, every new slot is born in the image, so every finished lane's slot is now unremovable (a second slot refused the same way). mu-validate may take it right after #894, or before it if #898 is smaller; its call. Until then, finished image slots stay in place, unreclaimed.
