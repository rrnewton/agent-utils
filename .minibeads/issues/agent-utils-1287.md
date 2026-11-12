---
title: 'retire-lock-narrow: retire-pending holds the exclusive project lock across whole removals'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
- lock-scope
created_at: 2026-11-11T10:52:39.000000000+00:00
updated_at: 2026-11-11T10:52:39.000000000+00:00
---

# Description

[watermelon-full-preview] A coordinator measured the configuration lock on a shared host on 2026-11-11, sampling `/proc/locks` every 2 s: one `wrkslots retire-pending --limit 8 --coordinator-authorized` held the exclusive flock on the project root continuously for 9+ minutes. A consumer's run-population lock takes the same flock, so non-blocking users lose and blocking ones time out (30-60 s bounds); the consumer's retention apply reported 60-69 items "authority unavailable" behind it, with a ledger backlog of 221 failed publications.

Cause: `retire-pending` delegates each candidate to the ordinary removal, whose locked section still runs the fleet-wide validation, the per-slot process-use census, the state-home reclaim, and the salvage derivation while it holds the exclusive mutation locks. The fenced tree's deletion already runs with the lock released (#894), and salvage facts are gathered before the lock (#1194), but eight back-to-back removals leave almost no gap between holds, so the lock reads as one continuous 9-minute hold. The same family: #1276 adopt-lock-narrow (validate and plan before the exclusive lock; no event log loaded under it), #1194 wrkslots-lock-scope (salvage and census before the lock), #1172 audit-lock-hold (readers verify history before the shared lock).

# Design

Give `retire-pending` the #1276 adopt-lock-narrow treatment:

- Validate and plan every candidate before taking any lock: the bounded batch's fleet validation, per-candidate salvage facts, remote containment, and process-use census all run with no lock held.
- Per slot, take the configuration lock only to re-check that slot's registry identity (its record identity and the journal fingerprint) and to commit its record or journal change (the retirement-attempt event, the basis records, and the finish journal plus path fence). The longest single project-root flock hold is then O(one record commit), not O(N slots of removal).
- Do the slow work — `git worktree remove`, fence and use checks, storage deletion, and salvage publication — outside the project-root flock, or under a per-slot lock that is not the project-root flock. Every existing safety check still runs; the census and salvage facts gathered lock-free are trusted under the lock only while their cheap fingerprints still match, and are re-derived there when they do not.
- If a re-check under the lock finds the slot changed, refuse that slot by name and go on to the next candidate; the batch neither blocks on it nor retries it inside the same run.
- Mirror the same narrowing in the Rust edition's removal write path (`rs/wrkslotsd/src/remove_write.rs`), which holds its mutation locks across the same validation, census, and salvage work, keeping parity. The Rust `retire-pending` command itself is not ported yet and stays with the Python edition.
- `wrkslots create` shares the fleet storage scan under the lock on one path: after #1276 a plain create computes the report lock-free and reuses it, but a create beside an outstanding journal deferred to the original locked path, which recomputed the fleet-wide storage report while holding the flock (measured on a shared host: two back-to-back creates held it 2 min 9 s and 2 min 35 s, printing dozens of retained storage inconsistencies). The journal classification still needs the lock; the storage report does not, so every round computes it lock-free and the locked path reuses it. Create gets its own counted lock-hold test for this path.

# Acceptance Criteria

- With N slots retiring, the longest single project-root flock hold is O(one record commit), not O(N slots of removal). Tested by counting or instrumenting lock-held operations, never wall-clock time: no `git worktree remove`, storage delete, fence check, salvage derivation, or fleet validation runs while the root flock is held during a `retire-pending` batch.
- A concurrent non-blocking flock attempt on the project root succeeds between slots of a `retire-pending` batch (tested).
- A slot whose registry identity changes between the lock-free gather and the locked re-check is refused by name, and the batch continues with its next candidate (tested).
- Every safety refusal of the ordinary removal still refuses through `retire-pending`: hold, handoff read, heartbeat and owner liveness, process use, salvage and unpushed work, and target storage consistency.
- The Python and Rust removal paths keep parity: the same lock-held operation counts hold for the Rust writer's fixture tests.
- A `create` beside an outstanding journal runs no fleet storage scan while holding the flock (counted, `py/wrkslots/tests/test_create_lock_scope.py`); its journal classification still runs under the lock as before.
- The existing lifecycle, lock-scope, prevalidated-mutation, and Rust differential suites pass, apart from the pre-existing #1267 quarantine class.

# Notes

Reported 2026-11-11 from a coordinator's lock sampling and a consumer's blocked ledger publications. Priority P0: the blocked consumer's ledger backlog grows while this lock is held. The owner directed counted (not timed) acceptance evidence, as in #1276.

## Resolution

[watermelon-full-preview] Landed on github/main as e6a7129a (final owner gate admitted at b0fee510; its only red was the untouched agentctl restore-drill test, green solo in 120 s). retire-pending validates and plans every candidate lock-free, commits one record at a time under the flock, and the Rust remove writer gathers the same way; create beside an outstanding journal reuses its lock-free storage report. #1332 proc-scan-bound (289cd3a8) removed the census-bound reds that had blocked this landing. The create provisioning remainder is #1361 create-provision-unlocked.
