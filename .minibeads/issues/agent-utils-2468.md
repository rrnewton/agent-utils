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
updated_at: 2026-11-28T11:00:00-05:00
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

## Root cause (cl-wrkdoctor, 2026-11-28)

Measured read-only on the reporting host's 9:37 AM ET run (/proc/locks, the unit's cgroup cpu.stat, /proc/PID/stat; no attach) and on scratch registries. Both edition surfaces read below; the consumer runs the Python edition.

- CPU is Git processes. The 8:37 run spawned 31,672 subprocesses; one `git rev-parse` through the host's default Git costs about 55 ms of CPU including the telemetry helper it leaves behind (2 ms through the system Git), which is the reported 1,884 CPU-s. The 9:37 run (17,585 subprocesses) used 793 CPU-s in its cgroup: 86 s in the Python process, 214 s in reaped children, the rest in the helpers.
- Where the processes come from (py/wrkslots/cli.py): the worktree-config walk (`_worktree_config_doctor_findings`) asked Git for a common directory before its seen-set test and re-walked the primary repository and every one of its submodules once per row; `_inspect_worktree_config` ran one `git config --get` per key; the shared-submodule walk (`_shared_submodule_doctor_findings`) probed the primary's submodule (`_shared_module_prep_state`, five processes) once per row and asked for the git dir and the common dir in two processes. A scratch registry shaped like the reporter's (seven submodules, four nested, private clones in every slot) needed 118 Git processes per populated row, all in those two walks; neither walk was cached across a retaken snapshot.
- The 8:37 run was two measurements: the registry changed during the first, the snapshot was retaken, and the retake spent 80 s + 37 s under the shared lock re-parsing the event log (2.6 GB, 101,826 files). Doctor applied its event-retention caps after its prewarm, so the prewarm kept the whole parsed history (5.45 GB resident against the 6 GiB scan limit); one appended event then dropped that chain, and every later reader of the full event tuple (pending operations, holds, owner releases, the event count) re-parsed the whole log because the memo caps (5,000 events, 256 MiB) are far below its size: eight full passes per run, two inside the locked snapshot.
- The Python lock scope itself was right (snapshot under the lock, measurement outside, #1247); the lock was held for the snapshot, which was slow. The Rust readers' `consistent_read` still ran its last measurement under the shared lock after three changed reads, which on a busy registry is a whole-scan hold.

## Fix

- One memoizing Git per doctor run for both walks (`_DoctorGitVcs`): each read-only question once; git dir and common dir from one `rev-parse`; each config file read with one `git config -z --list` (per-key answers unchanged, tested against the old per-key reads). Both walks cached by storage subject across retakes.
- Retention caps before the prewarm (doctor and status). Pending operations, holds, owner-release events, handoff publications and the event count are resumable streamed folds (`_resumable_event_fold`) remembered at a verified tip; the streamed registry replay advances them in its own pass, so a cold read parses the log once and a retaken snapshot reads only the events appended since.
- Rust `consistent_read` publishes its last lock-free measurement after the bounded retries, as the Python readers do; no reader measures under the shared lock.

## Measurements

Unit-scope cgroup CPU (the doctor and every process it leaves behind), host's default Git, --scan-timeout 1500, load1 about 220-235, 100-row scratch registry shaped like the reporter's (seven submodules, four nested, private clones in every slot; 1,500 findings):

| run | wall | cgroup CPU-s | subprocesses |
|---|---|---|---|
| before, events appended during the run | 1,059 s | 2,222 | 32,613 (three measurements) |
| after, events appended during the run | 90 s | 183 | 2,729 |
| before, quiet | 369 s | 756 | 11,141 |
| after, quiet | 100 s | 189 | 2,729 |

Doctor JSON byte-identical to main on the 5- and 100-row registries (timings, subprocess counts and heartbeat ages removed). Event-file parses per doctor with consumer-scaled caps and one append between prewarm and snapshot: 216 (eight passes) before, 27 (one pass) after, with only the appended event read under the lock. Per-spawn cost on the reporting host, 200 `git rev-parse` in a user scope: 55 ms CPU through the host's default Git (its telemetry helper included), 2 ms through the system Git.
