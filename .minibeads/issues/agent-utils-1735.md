---
title: 'wrkslots-remove-holds-state-lock-through-salvage: a long remove holds the registry lock through its slow phases'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-16T15:45:41-05:00
updated_at: 2026-11-16T15:45:41-05:00
---

# Description

[watermelon-full-preview] Consumer report (2026-11-16): `wrkslots remove <slot>` held the registry lock for more than thirty minutes (fenced phase, salvage), blocking three coordinator creates (`--wait-lock 900`) and the validate cleanup batches. A long remove should not hold the global state lock for its whole salvage phase.

Prior narrowing (#1194, #1276, #1287, #1367) already made plain and batched agent removes validate, census and publish salvage with no registry lock held, fence and journal under a short locked commit, delete the fenced tree unlocked, and re-take the lock only to record completion. The remaining remove shapes never got that treatment: in `_cmd_remove_in_scope` (`py/wrkslots/cli.py`) the non-prepared branch still runs `_removal_salvage` inside `_mutation_locks`, and the private-cleanup and scoped validation finishes still run their fenced deletion (`_finish_remove_paths` via `_complete_prepared_private_finish`) inside the completion lock. The Rust edition mirrors the same split (`prepare_remove` lock-free, but its non-deferred finishes hold `acquire_mutation_locks` across the same work).

# Design

- Name exactly which lock each remove shape holds across its slow phases, in both editions: the `_mutation_locks` hold (project configuration flock plus the `ACTIVE` registry locks) in Python, `acquire_mutation_locks` in Rust.
- Restructure so the slow I/O phases (salvage publication, archiving, fenced deletion) run under the per-slot fence or finish journal the removal already writes, with the global state lock taken only for short state transitions: re-check this slot's identity, write its journal/fence/record change, record completion.
- Crash safety is unchanged: an interrupted remove still leaves its finish journal and fence exactly as today, and the next remove or `recover` pass finishes or rolls it back the same way. No recovery behaviour changes.

# Acceptance Criteria

- A deterministic regression test, in both editions: while one remove's salvage is artificially slow, a `create` on another slot completes. Slowness is a stub blocked on an event or barrier the test releases; never a wall-clock budget.
- The existing lock-scope suites (`test_batch_lock_scope.py`, `test_retire_lock_scope.py`, `test_lock_scope.py`, `test_remove_lock_scope_rust.py`) still pass, extended to the newly narrowed shapes.
- An interrupted remove (killed mid-salvage and mid-deletion) still recovers exactly as before, covered by the existing interruption tests.

# Notes

Related: #1213 wrkslots-state-lock-deletion (remaining shapes: fence repair, private cleanup, scoped validation finish, interrupted-operation recovery), #1367 wrkslots-salvage-thin-bundle-fast. Land separately from #1736 wrkslots-live-seal-reported-as-interrupted when ready.

Addition (2026-11-16, from the owner-ordered cleanup): with `--no-lock-budget`, the finish runs the lsof and /proc use checks inside the journal (`_finish_remove_paths` > `_assert_slot_unused` > `_read_process_mountinfo`), so each finish is long and serializes the host. As part of this issue: move the use checks BEFORE the journal is written, and inside it re-check only something cheap (fence identity, owner liveness), keeping the proof sound.

Review additions (2026-11-16): grouped batch members must keep deferred deletion on the locked path too; `recover` must refuse a finish journal whose recorded remover is still alive (it currently stamps its own pid over the journal and would delete concurrently); a deletion refusal in a batch item rolls the fence back instead of leaving a journal that poisons later items; the private completion keeps the process and hold re-checks of the deferred completion. The Rust edition's `remove` already defers its deletion; its non-deferred `finish` still deletes under `acquire_mutation_locks` and is explicitly out of scope here (narrowed acceptance; a follow-up can mirror the defer). Noted, not fixed here: `reap-stray-worktrees --apply` takes no lock and treats checkouts under a fenced path as stray (pre-existing); file a follow-up if it is not fixed with this issue.
