---
title: 'wrkslots-doctor-cpu-lock: an hourly doctor on a 426-row shard burns 1,884 CPU-s in 15 minutes and holds the shared registry lock for the whole run'
status: open
priority: 1
issue_type: bug
assignee: cl-wrkdoctor
labels:
- wrkslots
- state-lock
created_at: 2026-11-28T09:35:00-05:00
updated_at: 2026-11-28T09:35:00-05:00
---

# Description

Reported by a consuming team's lead on 2026-11-28. That team's hourly doctor collector (`wrkslots --wait-lock 300 doctor --format json --scan-timeout 1500 --scan-memory-limit 6`, Python edition from the consumer's own agent-utils pin) ran at 8:37 AM ET for about 15 minutes and used 1,883.5 CPU-seconds. It was CPU-bound (the Python worker in state R, not waiting on a lock) and held a SHARED flock on `worktrees/ACTIVE.global.lock` for the whole run, so every exclusive wrkslots operation on the host (create, finish, remove) was blocked for those 15 minutes. Hourly, that is about 45,000 CPU-seconds a day and a quarter of every hour with no lifecycle mutations.

The published result of that run (rc 0, not partial) gives the shape: 426 active rows, 101,814 events, flat layout on a mounted worktrees image, 2,086 findings (742 shared-submodule-private, 733 shared-submodule-unprepared, 353 dead-owner, 155 row-without-directory), 31,672 subprocesses, total 852 s, with phase timings worktree_config 282 s, shared_submodule 183 s, snapshot 80 s, registry_tail 37 s, owner_liveness 29 s, image_locks 12 s, storage_census 10 s (about 220 s of the total is in no named phase).

Related: #1247 wrkslots-status-lock (read-only scans measure outside the lock, scan and memory bounds), #1706 wrkslots-doctor-speed (phase timings, `doctor --quick`, batched liveness), #1172 audit-lock-hold (snapshot under a short lock, measure outside it). #1247's fix says measurements never run under the lock; this report says the shared lock is held for the whole doctor run, so either a path still holds it or a later change reintroduced it.

# Design

1. Profile first, never guess: reproduce on a scratch registry of comparable size (hundreds of rows, a large event log, many shared-submodule checkouts) with cProfile in a scratch run, or `py-spy record --nonblocking` on the real interpreter with a SIGINT timeout (never a faulthandler dump). Separate the doctor process's own CPU from its children's (git subprocesses) and name the hot path with file:line, per phase, with operation and subprocess counts.
2. Lock scope: find where doctor acquires the shared `ACTIVE.global.lock` and how long it keeps it. Take it only for the registry snapshot (read and verify the state files), release it, then run every slow check (git probes, submodule census, liveness, storage census, image locks, tail scans) against the snapshot without the lock. Whatever doctor reports comes from that snapshot.
3. Fix the dominant CPU costs found in step 1 (for example batch per-repository git calls instead of per-slot ones, share per-run results computed more than once, avoid rescanning or re-verifying the event log), keeping findings byte-identical on the fixture.
4. Both editions: the Rust edition serves status/doctor/unpushed for projects opted into the readers routing; apply the same lock scope and the same algorithmic fix there where the surface exists.

# Acceptance Criteria

- The hot path is named with file:line and measured (CPU-seconds split between doctor and its children, subprocess counts per phase) on a scratch registry of realistic size; recorded in Notes.
- Doctor holds `ACTIVE.global.lock` only for the registry snapshot, in both editions where doctor exists.
- A test shows an exclusive wrkslots operation proceeds while a doctor scan is running (condition-based, no wall-clock assertion).
- CPU-seconds before and after on a comparable registry are recorded in Notes; findings are unchanged on the fixture.
- Existing doctor, status, audit, lock-scope and lifecycle tests pass.

# Notes

Report evidence at filing time; no measurement by this lane yet. Number pre-assigned by the coordinator (2026-11-28).
