---
title: 'remove-fence-seal: seal the fenced slot directory during remove deferred deletion, in both editions'
status: open
priority: 2
issue_type: task
created_at: 2026-11-10T16:37:45+00:00
updated_at: 2026-11-10T16:37:45+00:00
---

# Description

[opus 5.5] A slot being removed is renamed to a private fenced sibling (`.<slot>.fenced.<generation>.<hex>`) before the fenced process scan, but the fenced directory keeps its original mode. A same-user process that walks the parent directory can therefore `openat` into the fenced tree after the scan and before deletion, gaining a reference the scan never saw. This window exists today in ordinary `remove` (one item) in both the Python and Rust editions; the #1108 safety argument assumes nothing new can reach the tree after the rename.

Found while designing #1201 batch-grouped-fencing, whose approved design seals grouped members at mode 0o000 from the end of fencing until deletion starts. This issue brings the same seal to ordinary `remove`, so single-slot and grouped removal share one safety argument.

# Design

- After the rename fence, write `sealed: true` and `original_mode` to the item's journal, then chmod the fenced directory to 0o000. Recovery and rollback restore `original_mode` first; the steps are safe to repeat.
- Deletion runs with the seal in place, or unseals only for the deleting process's own walk, so no other process can open the tree in between.
- Image-backed slots (bind-mounted btrfs subvolume or loop mount): check that the chmod seals the mount root, and that wrkslots' own owned-mount scan, doctor and audit still work on a sealed fence.
- Both editions, Python and Rust, with the same journal fields. An older reader that meets `sealed` must refuse or stay hands-off, never delete or roll back.

# Acceptance Criteria

- A test where a same-user process tries to open the fenced tree after the scan: it is refused while sealed.
- Crash-recovery tests that restore the original mode from `prepared`, `fenced` and `removing` phases.
- Image-backed slot test (fixture) that seals and recovers the mount root.
- Rust and Python parity on the journal shape and recovery.

# Notes

Follow-up to #1201 batch-grouped-fencing (lead, 2026-11-10).
