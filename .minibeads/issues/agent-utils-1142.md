---
title: 'image-remove-fence-first: an image-slot remove still runs its slow lsof use check under the registry locks'
status: open
priority: 1
issue_type: bug
assignee: cl-wrkid
created_at: 2026-11-09T16:14:45.000000000+00:00
updated_at: 2026-11-09T16:14:45.000000000+00:00
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
