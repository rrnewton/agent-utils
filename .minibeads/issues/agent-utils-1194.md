---
title: 'wrkslots-lock-scope: gather salvage and census before the configuration lock'
status: open
priority: 1
issue_type: task
labels:
- wrkslots
- lock-scope
created_at: 2026-11-10T07:01:50.445436710+00:00
updated_at: 2026-11-10T07:01:50.445436710+00:00
---

# Description

wrkslots takes an exclusive flock on the project root directory (_locked_config in py/wrkslots/cli.py, entered by _mutation_locks) for the whole duration of mutating commands. ci-hub run_population_lock flocks the same directory, so every long wrkslots hold blocks validate-run run-record writes and the merge-queue donor entry probe (30s cap). Measured holds over 10s (sampling /proc/locks every 0.25s with py-spy dumps, 2026-11-09 night): retire-pending --limit 8 held 111.2s in _cmd_remove -> _reclaim_preconditions -> _assert_salvage_still_matches -> _salvage_candidate_facts -> git subprocess; remove held 91.6s and 15.8s on the same stack; recover-absent-validate-rows held 58.9s in _assert_absent_validate_processes_unrelated (privileged grep census); read-handoff held 17.0s and 11.5s. This contributed to a 40-minute merge-queue stall and one lost publication outcome write.

# Design

Gather salvage facts and the process census before taking the configuration lock, then revalidate cheaply under it: compare a fingerprint of HEADs, refs, worktree registrations and slot generation; if anything changed, release, re-gather and retry a bounded counted number of times, then refuse. read-handoff is read-only in its first phase and must not take the exclusive lock (shared lock or none, still consistent against a concurrent write-handoff). Keep every safety property: never weaken salvage, unpushed-work or census refusals; revalidation under the lock must still refuse when anything material changed; census rules (ENOENT/ESRCH gone; EACCES/EPERM with 200ms recheck else refuse) unchanged. Also check the Rust wrkslots (rs/) for the same pattern and note it here; Rust changes belong to its owner unless trivial.

# Acceptance Criteria

No configuration-lock hold over a few seconds for remove and read-handoff on a scratch project, measured with a lock-hold script before and after. Tests: a fake slow salvage step does not hold the lock (hold measured); revalidation refuses when a HEAD changes between gather and lock; read-handoff runs while another process holds the configuration lock. make validate passes for the touched paths.

# Notes

Rust check (report only, for the Rust wrkslots owner): rs/wrkslotsd does not flock the project root directory for salvage or census work. Its flock_wait/acquire_lock (rs/wrkslotsd/src/ensure_states.rs, reused by hold_write.rs) takes an exclusive flock on a per-state lock file for short state reads/writes, and its audit census is a reader. No multi-second salvage-under-lock pattern was found there; the long-hold pattern is Python-only (py/wrkslots/cli.py).

Census note (not a weakening): processes never take the configuration lock, so holding it during the process census never protected against a process starting in the slot later. The census judges the moment it runs, inside or outside the lock; moving it before the lock changes no refusal. The same argument is recorded in code comments at the gather sites.
