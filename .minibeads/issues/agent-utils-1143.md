---
title: 'slots-image-back-door: a project-image slot subvolume stays reachable at control/.slots-image/<subvolume> through its fence'
status: open
priority: 3
issue_type: bug
created_at: 2026-11-09T16:35:00.000000000+00:00
updated_at: 2026-11-09T16:35:00.000000000+00:00
---

# Description

Found during #1142 image-remove-fence-first's design review. With the btrfs
project image representation (`worktrees_image`), each slot is a subvolume of
the shared image, bind-mounted at the slot path. The remove fence
(`relocate_bind`) unmounts the canonical bind, renames the empty mount point and
binds the subvolume at the fenced path. The subvolume itself, though, stays
reachable at `control/.slots-image/<subvolume>` the whole time: before the
fence, during it, and after the decisive use scan.

A same-UID process that walks in through that path after the after-fence scan
and before the delete is not seen by the scan. That is the same residual as an
entrant through the fenced path, and #1142 neither creates nor widens it. But
the fence can never close this door, which per-slot images do not have (their
canonical name is gone after the fence, and the after-fence code refuses if it
reappears). A mode change on `.slots-image` does not help, because the entrant
has the same UID.

# Design

Not designed yet. Options to weigh:
- keep the image root mounted only where agents cannot reach it, for example in
  a private mount namespace of the root helper, with slots exposed only through
  their binds;
- before the decisive scan, move the subvolume under the fence (a btrfs subvolume
  rename into a fenced directory), so no stable path names it;
- record the remaining window and accept it, with the same argument as the
  fenced-path entrant.

# Acceptance Criteria

- A design that either closes the door (no path names a fenced subvolume after
  the decisive scan) or records why the residual is acceptable.
- If it is closed: a test where an entrant through the image-root path after the
  scan is refused or cannot reach the subvolume, so nothing is deleted under a
  live holder.

# Notes

The coordinator lead asked for it to be filed separately at P3 (2026-11-09).
