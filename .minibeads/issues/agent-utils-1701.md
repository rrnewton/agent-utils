---
title: 'wrkslots-recover-absent-memory-locks: recover-absent-validate-rows thrashes past its memory limit while holding every state lock'
status: open
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
