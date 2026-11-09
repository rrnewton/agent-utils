---
title: 'image-remove-fence-first: an image-slot remove still runs its slow lsof use check under the registry locks'
status: closed
priority: 1
issue_type: bug
assignee: cl-wrkid
created_at: 2026-11-09T16:14:45.000000000+00:00
updated_at: 2026-11-09T19:30:00.000000000+00:00
---

# Description

#1108 remove-lock-hold-starves-validate moved remove's /proc scans out of the
registry locks. A worktree-slot remove now holds them about 2 s (measured
2026-11-09, daytime, no added load). An image-slot remove still runs
before_image_fence (_observe_slot_use_once: lsof +D plus the direct scan) under
the locks. That took 11.4-12.2 s in five runs on a one-file scratch directory,
about 85% of the hold, and it grows with load and with checkout size (lsof +D
walks every file). So validation admission still waits behind image-slot
removals. The measurement is in #1108's Notes.

# Design

Fence first. Under the locks, fence the slot so no new process can enter it,
then release the locks. Run the lsof / direct-scan use check (and the remount
or unmount work) outside them. Re-take the locks only to record the result, and
keep the after-fence checks decisive.

Requirements from the lead coordinator (2026-11-09):
1. Cover BOTH image representations, whose remove and fence paths differ:
   - per-slot ext4 images (slot_representation: image), each mounted at its
     slot path;
   - the btrfs project image (worktrees_image): a subvolume per slot,
     bind-mounted at its path.
   Measure or reason about each.
2. A safety argument that after the fence no NEW opener can appear unseen:
   - exactly what the fence does to the path (bind unmount, rename, mode
     change);
   - why a use check outside the locks against the old mount cannot miss a
     process that entered between the fence and the scan;
   - that the after-fence checks stay decisive: refuse rather than delete on any
     doubt.
The design goes to the lead before code. Both editions, or a recorded parity gap.
The admission read-lock option is dropped for now.

# Acceptance Criteria

- An image-slot remove's lock hold no longer includes the lsof / direct-scan
  check, measured on both representations, or reasoned where root is needed.
- The design's safety argument is recorded in Notes and approved by the lead.
- Tests: a process entering between the fence and the scan is caught (refuse);
  a crash between the fence and the record leaves a state recover handles.

# Notes

Follow-up to #1108, filed by the devbig030 coordinator at the lead's request.

## Safety argument (approved by the lead, 2026-11-09)

Where the hold was: plain `remove` of a per-slot image slot was not a batch of
one, so it ran the ordinary lsof +D plus direct check TWICE under the locks
(once in remove, once at the fence); a remove-agent-batch image item ran it
once, just before the fence (2fd9dade, 2026-10-05). The btrfs project image
(worktrees_image) is not an image target: its bind slots already took the
batch-of-one path. The Rust edition refuses slot images in remove and has no
remove-agent-batch, so this is Python only, with no new parity gap.

Load-bearing premise: since b1b7196e (2026-10-18) the root helper never
attaches an image to a second loop device. While a device still holds the
image, it mounts that device (same block device, same ext4 superblock, same
st_dev and inodes); with two it refuses; it attaches a new device only when
none holds the image.

The fence (slotimage.relocate, still under the locks):
1. a non-lazy umount of the host mount (EBUSY while anything in this
   namespace or a propagated copy uses it: the fence refuses with nothing
   changed);
2. a release of copies in other namespaces by replacing the empty mount
   point (the kernel detaches mounts on a removed dentry everywhere, lazily
   when busy);
3. the record, then the rename to `.<slot>.fenced.<gen>.<hex>`;
4. a mount at the fenced path through the helper.
No mode change for agent slots.

After the fence, a process reaches the file system only:
- through a reference it already holds in a detached copy;
- through such a holder's /proc magic links;
- through the fenced path.
The first two exist only while the old superblock exists, which by the
premise IS the fenced mount. So every reachable inode has the fenced mount's
device. The checks after the fence read every process live, by path and by
(dev, ino), so a process that entered before they read it is found. An
entrant after the decisive scan is the residual every slot type has.

The checks stay decisive:
- `slotimage.sole_file_system_doubt` proves the premise for the fenced mount
  (kernel: its device is the one loop device holding the image, and no loop
  device holds an unidentifiable image; FUSE: exactly one fuse2fs) or the
  item refuses;
- the identity scan counts any reference on that device as use, whatever its
  inode (an unlinked file, a removed directory), which today's pre-fence lsof
  missed;
- an undecided scan falls back to lsof (always with FUSE), and lsof doubt
  refuses;
- a refusal rolls the fence back (a detached holder does not hold the host's
  fenced mount, so the unmount succeeds); a host entrant into the fenced path
  makes the rollback EBUSY, which leaves the removal to recover with nothing
  deleted.

FUSE and discard paths: unmount waits for the old fuse2fs to exit (a surviving
copy keeps it alive, so the fence raises), mount refuses while one serves the
image, and the discard phase mounts --fresh only when no device holds it.

btrfs: relocate_bind keeps the subvolume's st_dev and inodes (the file system
stays mounted), so the scan after the fence already matches any old-bind
holder; no change. The always-reachable `control/.slots-image/<subvolume>`
door is #1143 slots-image-back-door.

## Resolution

[opus 5.5] Image-backed agent slots are now image targets of the batch, and
plain remove of one is a batch of one, so no use check runs under the locks.
The pre-fence lsof (observe_image_before_fence) is gone. After the fence,
observe_fenced proves the premise (sole_file_system_doubt) and the identity
scan matches by whole device. The JSON report adds `image_fence_checks` and
`image_fence_check_seconds`; `image_lsof_checks` now counts image targets'
lsof fallbacks after the fence.

Measured on devbig030, daytime, one gate running, a one-file slot, with the
_locked wrapper of #1108's script (/tmp/cl-wrkid-1142/measure_image.py):

| Removal | Backend | Locks held before | After |
|---|---|---|---|
| plain remove | kernel | 30.5 s | 2.4 s |
| plain remove | FUSE | 26.3 s | 2.3 s |
| remove-agent-batch item | FUSE | 12.9 s | 2.5 s |

What stays under the locks: the directory re-check, the reclaim
preconditions without a scan, and the relocate (about 0.3 s). The btrfs
representation is reasoned, not measured (devbig030 has none).

Tests:
- a detached-alias holder (directory, and unlinked-file) is refused after the
  fence and the fence rolled back; with the whole-device match removed, the
  unlinked-file case deletes the slot under its holder;
- the fake reuse onto another device while the old one still holds the image
  refuses (kernel and FUSE);
- a unit table for sole_file_system_doubt;
- a crash at after-path-fence-before-journal and at after-path-fence is
  finished by recover;
- a budget spent before the fence leaves nothing fenced;
- plain remove of an image slot runs its lsof outside the locks.

