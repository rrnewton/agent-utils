---
title: 'wrkslots-recover-pruned-admin: recover cannot finish a finish target whose Git worktree admin dir is gone'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-07T01:55:00.000000000+00:00
updated_at: 2026-11-07T01:55:00.000000000+00:00
---

# Description

A validate-batch actor died mid-removal after the checkout's Git admin
registration (.git/worktrees/<name>) was deleted and before its files
were deleted: the canonical checkout stands with full files, its .git
file points at a nonexistent admin dir, the FINISH journal says
phase=prepared with removed=[], and the batch seal was restored by
recover. Every later `wrkslots recover` restores the seals, then refuses
in the finish retry because Git operations on the checkout fail with
"not a git repository" (verify_existing_worktree /
common_directory), and every create refuses "run wrkslots recover
first". The removal step order itself produces this midpoint
(remove_worktree deletes the registration as part of removing the
checkout), so recover must be able to finish from it.

# Design

In the finish/removal path, classify a checkout whose admin dir is gone
as abandoned when it carries no authored work (for a validate slot,
authored work is already excluded by construction; for other slots keep
the existing cleanliness judgments on whatever evidence remains),
retain the journal/seal evidence, and finish the removal instead of
refusing on Git provenance. Never hand-edit journals; no new flag that
skips the unused/cleanliness proofs.

# Acceptance Criteria

- A fixture reproducing this state (finish journal phase=prepared,
  removed=[], admin dir absent, checkout files present, seal journal
  present) recovers to completion: slot removed, journals resolved.
- A checkout in that state WITH authored work still refuses and is
  retained.
- Existing finish/recover/seal tests keep passing.

# Notes

Filed from a live consumer harness blockage (every wrkslots create
refused). Investigation was read-only; the harness state is untouched.
