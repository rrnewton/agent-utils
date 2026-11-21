---
title: 'wrkslots-e2e-stress-teardown-leaks-mounts: success-path teardown deletes mounted image files and strands loop devices'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T09:55:00.000000000-05:00
updated_at: 2026-11-21T09:55:00.000000000-05:00
---

# Description

On a kernel-backend host (imagehelper through sudo -n, ext4 loop images), `python3 py/wrkslots/tests/e2e_stress.py --workers 2 --seconds 2 --seed 1` passes all 6 scenarios, then its success-path `shutil.rmtree(root)` fails EBUSY because scenarios leave slot.img and state.img mounted; worse, the rmtree deletes image files while they are mounted, stranding loop devices (8 leaked mounts in the reported run: forgotten, descendant, stress00, stress01). Detach must go through wrkslots itself (`wrkslots image unmount`), never hand umounts, and no file under a live mount point may be deleted.

# Design

In `main()` of `py/wrkslots/tests/e2e_stress.py`: after the scenarios, on success AND failure paths, detach every scenario project's slot images via `wrkslots --project-root <proj> --machine e2ehost image unmount <slot> --slot-type <type>` (enumerated from `worktrees/slot-images/<type>/<slot>/`). Then verify via `/proc/self/mountinfo` that nothing under the scratch root is still a mount point: the rmtree runs only when no mount remains, otherwise the root is retained and the teardown is reported as failed (exit 1) naming the remaining mount points. Already-orphaned loop devices from earlier leaks are recovered by the product helper tracked separately (#2050, mu-wrkfix); this fix only stops new leaks.

# Acceptance Criteria

The kernel-backend run above exits 0 with no mounts left under its scratch root and no loop devices leaked; a forced unmount failure (slot in use, exit 3) retains the root, names the mount point, and deletes nothing under it. `py/wrkslots/tests/test_e2e_stress.py` still passes.

# Notes

[watermelon-full-preview] Filed and fixed in the #2043 lane at the coordinator's instruction; cleanup commands for already-leaked mounts were sent to the coordinator for the consumer lane (image status, image unmount per slot as the scratch owner, rerun after stopping any holder named by status; never delete image files while mounted).
