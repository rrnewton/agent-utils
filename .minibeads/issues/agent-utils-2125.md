---
title: 'wrkslots-history-read-outside-config-lock: read the event history before taking config.lock, then recheck under it'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-22T13:15:00-05:00
updated_at: 2026-11-23T22:02:59-05:00
---

# Description

Commands that replay the full event history (about 95,000 files on one consumer host) do so while holding worktrees/config.lock: recover-absent-validate-rows holds it about 2.3 minutes per call for rows that pass its pre-checks, and slot moves hold it 5 to 6 minutes. Back-to-back calls starve slot heartbeats waiting 300 seconds, which then get refused. #2088 removed the lock hold only for rows that refuse early.

# Design

Prove-then-recheck: read and replay the history outside the lock, take the locks, verify the snapshot revision and event-log tip are unchanged since the read (cheap), and only then mutate; if anything moved, redo the read (bounded retries) or fall back to the locked path. Apply it to recover-absent-validate-rows and to slot moves first, then to other long locked replays. Measure lock hold before and after.

## Owner rule (binding on all registry-lock work)

Only ever do quick, constant-time things while holding one of these
locks. If there is a linear-time thing to do, do as much prep work
ahead of time as possible: linear work happens outside the lock and
is validated under it by an O(1) generation/tip check, redone if it
changed. #2127 (per-slot locks) reuses the `_history_tip_proof`
pattern introduced here instead of re-deriving it.

# Acceptance Criteria

- Lock hold for a passing recover-absent call and a slot move drops to seconds on a large history fixture; a concurrent mutation between read and lock is detected and handled; tests for both.

# Notes

Follow-up to #2088; event-store compaction stays in #1123. Filed by the coordinator.

## Refinement (mu-lockread, 2026-11-22)

Implemented prove-then-recheck for `recover-absent-validate-rows`
(Python and Rust editions) and for the slot move
`relocate-repository` (Python-only command):

- `_history_tip_proof` captures, per machine, the event index tip
  (sequence + digest, proven against the chain tip and the ACTIVE
  snapshot revision via `_scoped_status_state`), the ARCHIVED
  revision, and the on-disk journal names. No new persisted fields.
- The history replay and plan run with no lock held; the locks are
  taken only when the proof is unchanged, and the mutation then
  skips its replays (prepared states thread through
  `_plan_absent_validate_rows` / `_recover_absent_validate_rows`;
  the batch-removal change guard and the hold check use the proven
  state and the scoped index instead of folding the log).
- A moved history is re-read for up to 3 rounds, then the command
  falls back to the previous locked path unchanged. An outstanding
  journal (the resume path) always takes the locked path.
- The Rust edition mirrors this: the fold runs before
  `acquire_mutation_locks`, gated by the same tip/revision proof,
  with the locked path as fallback.

Lock hold on a synthetic 20,000-event history (same fixture, base
vs this change): 35.8 s under the locks before, 0.8-3.0 s after
across runs (total command 35.8 s -> 14-22 s). At the reported
95,000-event scale the before number extrapolates to the observed
~2.3 minutes.

Tests: `py/wrkslots/tests/test_recover_absent_outside_lock.py` -- a
concurrent write between read and lock is detected and the read is
redone (recover and relocate); a passing row on a padded history
performs no history replay while the locks are held.

## Resolution

Landed on main as 037494e9 (verified-overlap landing; replayed
tree 312e8a6e). recover-absent-validate-rows (Python and Rust) and
relocate-repository now replay the event history outside the
registry locks and mutate only when the O(1) event-index tip proof
is unchanged under them, with bounded re-reads and the previous
locked path as fallback. Follow-up lock work continues in #2146
(fenced finish strands the batch seal) and #2127 (per-slot locks).
