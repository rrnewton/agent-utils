---
title: 'validate-mount-fence: unmount an image-mounted validation checkout before the path fence'
status: open
priority: 0
issue_type: bug
assignee: mu-fence
labels:
- wrkslots
created_at: 2026-11-11T16:14:36+00:00
updated_at: 2026-11-11T16:14:36+00:00
---

# Description

[watermelon-full-preview] URGENT, owner tier: merge-queue/wrkslots defects are same-hour. This cost the reporting host's merge-queue its donor: its hold is up and queue jobs now run on one remaining host only.

validate-run setup on the reporting host refused with exit 75 (job 20261111T160025634-59d765ba592a-7ab0c5a0):

    wrkslots did not confirm removal of validation checkout worktrees/validate/validate-fresh-59d765ba592a-404769-b4d6434a: REFUSED: cannot establish path fence

This is the second such refusal since last evening. The checkout path is itself a mount point: `findmnt` shows `/dev/loop5[/validate-fresh-59d765ba592a-404769-b4d6434a]` (btrfs subvolume of the project worktrees image) and its `st_dev` differs from its parent's. Confirming read-only (the evidence tree is untouched): the mount is still in place, and `rename(2)` of a mount point fails with EBUSY (errno 16), reproduced against a scratch bind mount. The removal fence is `rename(original -> fenced)` in `py/wrkslots/cli.py` (`_begin_or_resume_path_fence` -> `_rename_slot_path`) and a plain `std::fs::rename` in `rs/wrkslotsd/src/remove_write.rs`. The image re-mount handling applies only when the target is in `finish.agent_batch.image_targets`; the validate-checkout removal sits outside that batch, so an image-mounted validation checkout reaches the rename still mounted and the fence refuses. The Rust fence performs no mount relocation at all.

# Design

- Before the fence rename, in both editions (Python `cli.py` and Rust `remove_write.rs`, keeping the differential agreeing): when the fence source is itself an image mount (a worktrees-image bind, a slot image, or a direct subvolume mount of the project image), unmount it or relocate it as an image target, then fence and remove as today.
- If the unmount itself fails, fail closed with a clear refusal naming the mount point and the underlying error; never rename a mounted path and never force past EBUSY.
- Check whether agent-slot removal outside `image_targets` has the same gap and cover it the same way.
- Reproduce first with a fixture: an image-mounted (btrfs or loop/bind) validation checkout in a temp project, or the existing image test helpers, showing the EBUSY fence refusal before the fix and a clean fence-plus-removal after it.

# Acceptance Criteria

- A fixture with an image-mounted validation checkout fences and removes cleanly in both editions; before the fix it reproduces the `cannot establish path fence` EBUSY refusal.
- An unmount failure refuses with a message naming the mount, in both editions, and leaves the checkout, its mount, and its records untouched for a retry.
- Agent-slot removal outside `image_targets` is checked; any identical gap is fixed the same way, or the check's negative result is recorded here.
- The gate passes at owner priority and the fix lands on `main`; the reporting host's pin can then be deployed by its owner.

# Notes

Reported by the originating validator's lead 2026-11-11 11:07 AM ET; job 20261111T160025634-59d765ba592a-7ab0c5a0. Evidence is preserved in place on the reporting host: nobody unmounts, recovers, removes, or releases locks on that checkout without the evidence owner's explicit OK. mu-retirelock's #1287 gates in the same remove code: rebase onto github/main right before the gate, and if #1287 lands first, rebase over it and re-verify the combined tree. Known main red, unrelated: the restore-drill reds (fixtures vs #1216); an untouched red needs one local solo pass.
