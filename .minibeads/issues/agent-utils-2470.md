---
title: 'wrkslots-remove-private-submodule-checkouts: remove refuses a released slot whose only extra checkouts are its own private submodule checkouts'
status: open
priority: 3
issue_type: bug
assignee: cl-wrkdoctor
labels:
- wrkslots
- reclamation
created_at: 2026-11-28T09:40:00-05:00
updated_at: 2026-11-28T09:40:00-05:00
---

# Description

Reported by a consuming team's coordinator on 2026-11-28: a released slot cannot be removed. Removal stops on two nested checkouts inside the slot and points at `adopt-checkout`, which refuses both: they are not top-level checkouts but the slot's own submodule checkouts, with Git directories under `<superproject>/.git/worktrees/<slot>/modules/` (Git's default for submodules of a linked worktree). `git submodule deinit` is not a way out: in a linked worktree it writes the SHARED repository configuration, which other slots and the primary checkout read.

So a slot whose submodules were initialized privately (the default `git submodule update --init` in a linked worktree) has no supported removal path once it is released, even when every submodule checkout is clean with HEAD at the superproject's recorded gitlink.

# Design

Design note to the coordinator before any code. Direction:

- Removal recognises the slot's own PRIVATE submodule checkouts (a `.git` file naming a Git directory inside the superproject worktree's own Git directory, nested ones too) as part of the slot and deletes them with it, after the same cleanliness proof as the superproject: clean outside configured cache paths, HEAD equal to the gitlink the superproject records (or contained in a remote ref), no unpushed local branch work.
- Reuse the private-submodule classification of #2392 (PRIVATE / SHARED / EMBEDDED / UNCLASSIFIABLE, total over the index's gitlinks) so the validate teardown and agent removal agree on what a slot's own submodule checkout is; SHARED, EMBEDDED and UNCLASSIFIABLE keep refusing by name.
- Never write shared Git configuration: no `submodule deinit`, no `git config` on the common directory; the private module Git directories go with the superproject's worktree administrative directory, as Git itself would remove them.
- Rust parity where the Rust edition reports the same refusal.

# Acceptance Criteria

- A released slot whose submodules are private, clean and at their gitlinks is removed by `wrkslots remove` with no extra command, and the shared repository configuration is byte-identical before and after (test).
- A dirty private submodule checkout, one with unpushed local commits, or one whose HEAD differs from the gitlink still refuses with a message naming the submodule and the reason (tests).
- SHARED, EMBEDDED and UNCLASSIFIABLE submodule checkouts keep refusing by name, with the same classification as #2392's teardown (one shared classifier, tested once for both callers).
- Existing remove, adopt-checkout, shared-submodule and lifecycle tests pass.

# Notes

Report evidence only at filing time; this lane has not inspected the reporting host and will not (reproduction on a scratch project only). Number pre-assigned by the coordinator (2026-11-28). Related: #2392 (validate-slot unit-proof teardown, private-submodule classification), #1206 (shared submodules).
