---
title: 'wrkslots-orphan-loop-detach: a sanctioned detach for a loop mount whose backing image was deleted'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T11:45:00-05:00
updated_at: 2026-11-21T11:45:00-05:00
---

# Description

A test teardown deleted slot images while they were still loop-mounted. imagehelper's umount verifies the image by device and inode, which no longer exist, so the loop devices are stranded with no sanctioned way to detach them.

# Design

[watermelon-full-preview] Add `wrkslots image detach-orphan --device /dev/loopN --image <expected-path> --mount-point <path>`, run through the same privileged-argv helper (`imagehelper.py` `detach-orphan DEVICE IMAGE MOUNT-POINT`). Act only when the loop device's backing file shows ` (deleted)`, the stripped path is exactly `--image`, `--image` is `slot.img`/`state.img` under a registry's `slot-images/<type>/<slot>/` tree, and the mount table shows that literal device mounted exactly at `--mount-point` (for `state.img` the image directory's `state/`; for `slot.img` the recorded location when the record survives). Then `umount2(UMOUNT_NOFOLLOW)` and `LOOP_CLR_FD` on that literal device. Refuse a non-deleted backing file (pointing at the normal umount), any path/mount mismatch, and a busy mount (EBUSY named, never a lazy unmount). The receipt names the exact device, image path, and mount point acted on. No persisted-format or lock change; the Rust CLI keeps refusing `image detach-orphan` by name until its phased wiring, sharing the same helper file when wired.

# Acceptance Criteria

- The behaviour described under Design, covered by a deterministic regression test (condition-based, no wall-clock budgets): `test_imagehelper_orphan.py` accepts the sanctioned slot/state shapes and refuses each single-condition violation; the privileged act itself is exercised only on the owning lane's own scratch images.
- The exact operator command is written into the user guide (`common/docs/wrkslots/USER_GUIDE.md`).

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
