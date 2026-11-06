---
title: 'wrkslots-recover-verifies-checkouts: recover and create never report success with an empty or unclean checkout'
status: closed
priority: 1
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-05T10:57:10.526253628+00:00
updated_at: 2026-11-05T10:57:10.526253628+00:00
closed_at: 2026-11-06T19:18:43.000000000+00:00
---

# Description

After a refused `wrkslots create`, `wrkslots recover` registered the slot and exited 0, but two of its submodule checkouts were left broken. Each had HEAD set and an EMPTY worktree, with every file staged as deleted (1,312 and 9,036 files). Nothing in recover's output said so.

The operator found it by hand and repaired it with `git reset --hard HEAD`. This was seen on a consumer project on 2026-11-05, about 6 AM ET.

A slot that reports success while its checkouts are empty is worse than a refusal: an agent starts work in it and its first commit deletes the tree.

# Design

- Before reporting success, `recover` and `create`'s completion verify every checkout of the slot, submodules included recursively:
  - HEAD resolves;
  - the worktree is populated;
  - `git status` is clean, apart from the slot's own allowed cache paths.
- When a checkout is unpopulated (an index or worktree that disagrees with HEAD wholesale, as after an interrupted checkout), repopulate it, only in a worktree that this slot creation owns and that holds no authored work: `git checkout -f HEAD` / `git reset --hard HEAD` scoped to that checkout. Then re-verify.
- Otherwise fail with a clear state that names the checkout, the counts of missing/staged-deleted files, and the remedy. Never report rc 0 for an unverified slot.
- Both editions.

# Acceptance Criteria

- A test interrupts a submodule checkout mid-populate (for example by killing the checkout, or by injecting a failure after the index is written). `recover` then either repopulates and verifies, or refuses naming the checkout; it never exits 0 with an empty checkout.
- A test with a checkout holding authored changes: it is not reset, and it is reported.
- `create` performs the same final verification.

# Notes

Reported by the lead on 2026-11-05 at 06:00 AM ET, from a consumer team (P1). Assigned to mu-salvage, right after #875.

Moved from mu-salvage to the new Muse ultra wrkslots lane mu-wrkslots on 2026-11-05 at 5:50 PM ET (the owner lifted the Muse lane cap; the lead asked for a dedicated wrkslots lane).

## Resolution

Landed in the mu-wrkslots stack at 84585684. Create and both recover-create paths verify every checkout recursively before reporting success: wholesale-unpopulated checkouts are repopulated from HEAD and re-verified, authored tracked changes are named and never reset, and untracked post-provision hook output is allowed (gate finding).
