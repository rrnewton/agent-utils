---
title: 'wrkslots-recover-absent-memory-locks: recover-absent-validate-rows thrashes past its memory limit while holding every state lock'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-16T03:09:55.000000000+00:00
updated_at: 2026-11-16T03:09:55.000000000+00:00
---

# Description

Reported by a consuming team's lead on 2026-11-15. `wrkslots
recover-absent-validate-rows --apply`, run inside a unit with a 1 GiB
memory.high, thrashed for about 75 minutes: D state in
__mem_cgroup_handle_over_high, 2.43 GB swapped, no user CPU and no syscalls
over a 169 s sample. The whole time it held the host's ACTIVE journal lock,
the global lock and the state-root flock, so every wrkslots command on the
host was blocked.

Two defects:

1. Memory: recovering absent validate rows should not need more than a
   small, bounded amount of memory. Something loads far more than it needs
   (whole journals, every row's history, archive manifests, object listings
   or bundle contents held in memory).
2. Lock hold under resource pressure: a command holding the host-wide
   locks has no bound and no way to give them up, so one slow or starved
   process stalls the host.

# Design

- Measure first: peak RSS and the allocation profile of recover-absent on
  a large registry (fixture sized like a busy host). Name what dominates.
- Stream or bound it: process rows one at a time or in bounded batches,
  read journals incrementally, and never hold per-row data for all rows.
  Document an expected peak (for example tens of MB for thousands of rows).
- Do the expensive per-row work (archive checks, scans) WITHOUT the
  host-wide locks where the design allows: plan outside the lock, then take
  the lock only to re-validate and commit each row's change.
- Fail fast: if the process cannot make progress (for example, a memory
  limit is near or a per-row step exceeds a documented bound), it stops,
  releases every lock, leaves a resumable journal, and exits non-zero with
  a clear message naming the row it stopped at. A rerun resumes there.
- Record the lock hold time in the command's output and journal, so a long
  hold is visible.

# Acceptance Criteria

- A large-registry fixture: recover-absent --apply stays under a documented
  peak RSS (test or measurement recorded in the commit).
- Locks are held only around each row's commit; a test shows another
  wrkslots command can run while recover-absent is mid-scan.
- An injected stall or memory-pressure signal makes it stop, release every
  lock and exit non-zero with a resumable journal; a rerun completes.
- Checks wait on conditions, not wall-clock budgets.

# Notes

Related: #1700 (status must report an interrupted mutation), #1276
(adopt and create validate the whole registry while holding the exclusive
project lock), #1554 part A3 (refused recover-absent journals).

## Measurements (2026-11-18, before)

Real `recover-absent-validate-rows --apply` in a subprocess,
ru_maxrss (about 50 MiB run variance): 200 absent rows, empty
event log: 175.2 MiB, 21.8 s. 1024 rows (the input cap), empty
log: 165.4 MiB, 28.6 s. 2 rows with 20,000 chained events
(4x3 MiB payloads): 268.6 MiB. 2 rows with 50,000 events
(10x3 MiB payloads): 216.0 MiB. Row count does not dominate;
whole-history loads do (every state load folds the chain, and
the batch holds every item's full record plus archive entry).

## Resolution

- Archives now land in bounded chunks of 64 with the journal
  cursor advancing per chunk (`_recover_absent_validate_rows`):
  a process killed by a memory cap mid-batch leaves a resumable
  journal whose durable archive prefix the next run revalidates
  and continues. The durable-prefix invariants are unchanged.
- The command reports `lock_seconds` (registry lock hold time)
  in its JSON outcomes and human output.
- Streamed verification by snapshot readback was tried and
  reverted: the evidence the final check must match
  (`absent_validate_recoveries`) exists only in the event fold,
  not in snapshot files, so the fold stays. #1794's streamed
  fold serves read-only bounded scans; recovery is a mutation
  on the parse-once memo, and no second fold was written.
- Not done, by design: releasing the registry locks between
  per-row commits. The batch's safety proof is the durable
  prefix (archives form a prefix, ACTIVE removals form a
  prefix, both proven under one lock); per-row release would
  need a new journal protocol, filed as follow-up if the
  measured lock hold (now reported) still bites.

Tests: `py/wrkslots/tests/test_recover_absent_bounded.py`
(lock_seconds reported; chunk interrupt then replay completes),
plus the existing absent-row lifecycle tests.
