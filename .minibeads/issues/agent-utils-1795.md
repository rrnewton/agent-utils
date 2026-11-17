---
title: 'wrkslots-no-lock-budget-remove-holds-locks-through-salvage: a finisher remove starves every heartbeat in the project'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-17T04:33:45-05:00
updated_at: 2026-11-17T04:33:45-05:00
---

# Description

[watermelon-full-preview] Consumer data point (2026-11-17): a `remove --no-lock-budget` of a large slot (about 720 rescue tips) held the project and configuration locks for over an hour (salvage took 50 minutes), and every live lane's heartbeat in that project was refused ("configuration lock is busy"), so slot TTLs lapsed.

`heartbeat` takes `_mutation_locks` like every per-slot write. Ordinary, batched, and prepared removes no longer hold those locks through salvage (pregather plus the lock-free preparation of #1194/#1287/#1367, and the out-of-lock finishes of #1735), but the `--no-lock-budget` shape is excluded from the single-item preparation (`single_item` in `_cmd_remove_in_scope` requires `not args.no_lock_budget`), so it still runs `_removal_salvage` inside `_mutation_locks` -- and on a slot with hundreds of rescue tips the in-lock salvage work (fingerprint re-checks, bundle verification) is itself the long hold, even with the lock-free pregather in front of it.

# Design

- Give the `--no-lock-budget` remove the same lock-free salvage treatment as the prepared paths: salvage publication before the registry locks, only cheap fingerprint revalidation under them, or an equivalent split where the locked section never runs per-tip salvage work.
- Heartbeats and other per-slot writes of OTHER slots must complete while such a remove salvages; the removal's own slot keeps the existing proofs (its deciding census still runs at its fence).
- Keep the finisher's unbounded-by-budget contract: no new item budget appears; the change is lock scope only.

# Acceptance Criteria

- A deterministic test: while a stubbed salvage of one slot is blocked on an event, a `heartbeat` for a different slot completes; then the remove finishes normally. Condition-based only (event/barrier), no wall-clock budget.
- The existing lock-scope suites (test_lock_scope, test_batch_lock_scope, test_retire_lock_scope, test_remove_fenced_lock_scope) still pass, extended to the no-lock-budget shape.
- A remove interrupted during its lock-free salvage still recovers exactly as today (journal/fence behaviour unchanged).

# Notes

Related: #1735 wrkslots-remove-holds-state-lock-through-salvage (the lock-scope program this completes), #1755 wrkslots-wait-lock-covers-busy-finish-journal. From the mu-removelock lane; the reported shape is the one #1735's prepared paths do not reach.
