---
title: 'wrkslots-e2e-concurrent-writer-tmp-visibility: a heartbeat sees a sibling writer in-flight event tmp file under load'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- lifecycle
created_at: 2026-11-26T12:30:00+00:00
updated_at: 2026-11-27T17:54:43-05:00
---

# Description

[watermelon-full-preview] Found 2026-11-25 while gating #2168: `wrkslots/tests/test_e2e_stress.py::test_real_process_and_git_invariants` (seed 7) failed twice in full gates (python.wrkslots.shard-10) with a concurrent-stress heartbeat refusing "partial atomic update found: EVENTS.e2ehost/00000000000000000014.json.tmp.<rand>; preserve it and use 'wrkslots recover'". A heartbeat scans the slot's event directory while a sibling worker is between writing its event tmp file and renaming it into place, and treats the in-flight tmp as corruption. Retained evidence from the first failure: /tmp/wrkslots-e2e-7-1ru8158c (events 01-10 landed; 14 was mid-write).

# Design

Decide where the tolerance belongs: a heartbeat (a read-mostly liveness probe) arguably should ignore or retry past a tmp file that a live sibling is about to rename, while a recovery or audit path should keep refusing a stale tmp. Any tolerance must be bounded (a tmp older than some age is still corruption) and must not weaken the atomic-update check for committed event files. No new persisted field is expected; any new persisted field would be reported to the coordinator before gating and gated by `writer_ceiling >= 3` (the #1788 mechanism) or not persisted.

# Acceptance Criteria

- The exact test passes 5x alone on the fixed tree and on the base it was reported against, at recorded load, and in a full gate shard without the visibility refusal.
- A genuinely stale tmp (no live writer) still refuses with the existing remedy.
- Existing heartbeat/event suites pass.

# Notes

Triaged under the flake rule with 5/5 solo passes on both the reported tree and main (load1 115-154 recorded); cleared as a load-amplified race, not a product regression, and filed here as the real follow-up. Whether current main already changes this behaviour is checked before the fix and recorded in the resolution.

## Production sighting and cause (2026-11-27)

[Claude Opus 5.5] Raised to P1: a consumer at pin 132dd98d saw a heartbeat refused, rc 3, "partial atomic update found: .../EVENTS.<machine>/<sequence>.json.tmp.<pid>.<hex>; preserve it and use 'wrkslots recover'". The file was another slot's in-flight heartbeat write. Its writer exited, the file vanished, and the event landed intact; the next event names its digest as previous.

Cause, unchanged on main: the lock-free prepare of `heartbeat` and `transfer-owner` runs `_refuse_partial_state`. `_run_prevalidated_mutation` reports a prepare refusal when the registry identity reads the same before and after. A peer that holds the locks between its fsync and its rename leaves that identity unchanged, so its file is reported as a leftover. The Rust editions of both commands run the same scan lock-free and refuse outright.

Fix: every writer of these temporary files holds the exclusive mutation locks while the file exists (audit below), so a file still present under those locks is a leftover, while one seen without them proves nothing.
- Python: `_refuse_partial_state` raises `_PartialUpdateFound` (a `StateError`, same text, same rc 3). `_run_prevalidated_mutation` defers that lock-free finding to its locked round, as it does an outstanding journal.
  - If the identity changed (the peer renamed or removed its file), a new round validates.
  - If it held still, `commit(None, None)` runs the command's original validation under the locks, which still refuses the leftover with the same remedy.
- Rust `heartbeat` and `transfer-owner`: a lock-free finding takes the same locks the commit takes and re-checks there. Still present: the refusal stands. Gone: the prepare goes on.
- No age threshold, no pid guessing, no persisted field.

Prevalidated commands (every caller of `_run_prevalidated_mutation`): `create`, `adopt`, `transfer-owner`, `heartbeat`. `create` and `adopt` already judged partial state only under their locks; `transfer-owner` and `heartbeat` judged it lock-free and now defer. test_partial_update_in_flight.py proves, for each of the four, that a stale file still refuses with the existing text, nothing written and the file kept, and that the refusal reaching the caller was judged with the exclusive lock held.

## Writer audit (2026-11-27)

Both editions. These are the writers of `EVENTS.*/*.json.tmp.*` and of the other temporary files `_refuse_partial_state` scans (shards, journals, seals, handoffs). The audit is a static call graph of every chain from a command to such a write that passes through no `_mutation_locks` or `_locked` scope; every surviving candidate was then read by hand.

Python: every event file is written by `_EventWriter.append` (through `_write_event_file`, `_write_active_state`, `_write_archive_state`, `_write_journal` and the command writers) or by `recover-tail-event` (under `_mutation_locks`). Candidates checked by hand:
- The prevalidated `commit` callbacks: called only inside `_mutation_locks` in `_run_prevalidated_mutation`. Locked.
- The deferred and private finish deletion (#1735) runs lock-free by design. But the only journal or event write it can reach, the Eden branch of `_finish_remove_paths`, is unreachable there: deferral excludes Eden records (`not _record_is_eden(final_record)`).
- `remove` of a validation batch releases its own seal without the lock when its cleanup lock refused and nothing was deleted (#1744). It rewrites a seal only to upgrade a legacy three-field identity, which a seal this build just published never has.
- Closed here: heartbeat's lock-free validation used to reach `_recover_dead_actor_journals` through the cleanup-journal disjointness check and `_assert_no_journal`. That function documents "Called under a mutation lock". It recovers a dead actor's recover-absent-* journal: it appends events and rewrites the shard. The prepare defers first when any journal is outstanding, so the recovery fired only if such a journal appeared, and its actor died, between those two reads.
  - Under this issue's fix, the partial-state judgement could only fail safe in that window. Another command holding the lock could see the recovery's live temporary file and refuse falsely (exit 3, nothing written), which is the pre-fix behaviour. It could never lose a write.
  - The unlocked recovery itself could race a locked writer on the event chain, which is a separate pre-existing hazard. So the path now takes the lock:
    - the lock-free caller passes `recover_dead_actors=False` and never recovers;
    - a journal found there refuses that round, and the identity change sends the next round to the locked body;
    - the locked body recovers dead actors' journals before it reads the record. Before, it recovered inside the cleanup check after reading, so the heartbeat refused "active state revision must advance exactly once". Before #2353 it refused earlier still, "slot ... is active and archived", and never recovered.
  - With this, every production writer holds the mutation locks while its temporary file exists.

Rust: every `append_event` and `atomic_write_json` call site follows `acquire_mutation_locks*` or `acquire_lock` in its function or its caller, except one. `release-frozen-incomplete` (fixture-only, `WRKSLOTSD_FIXTURE_WRITER=1`) writes its journal and event with no lock, while the Python edition hands that removal to the locked recover path. Reported for a follow-up.

So the argument holds for every production writer. The one remaining exception is the Rust fixture-only writer, `release-frozen-incomplete`, which needs `WRKSLOTSD_FIXTURE_WRITER=1` and is never run against the live registry.

Other lock-free partial-state checks, outside the prevalidated commands and unchanged here, can refuse a live peer's file the same way:
- Python: `classify-create-journals`, the lock-free pre-check of `recover-absent-validate-rows --apply`, `classify-ownerless-validate-batch`.
- Rust: `audit`, `classify-create-journals`, `classify-ownerless-validate-batch`, `retirement-queue`, `unpushed`, and the `release-frozen-incomplete` pre-check.

Reported for a follow-up.

## Tests (2026-11-27)

test_partial_update_in_flight.py (19 cases, conditions only, never a wall-clock budget; the peer releases once `/proc/locks` shows the command queued behind it):
- (a) For each prevalidated command, a peer's file gone by the time the command takes its locks never refuses.
- (b) For each prevalidated command, a stale file still refuses under the lock.
- (c) For heartbeat and transfer-owner in both editions, a real peer process holds the registry flocks and an event temporary file, and the command queues behind it and succeeds.
- The Rust stale-file refusal for both commands, with the Python edition's identical text.
- For each prevalidated command, every dead-actor journal recovery runs with the exclusive lock held.
- A heartbeat beside a dead actor's recover-absent journal recovers it under the lock and then succeeds.

Red on the base (08cc496e):
- (a) and (b) for transfer-owner and heartbeat, and all four (c) cases, each with the production text;
- the heartbeat case of the lock-held recovery check;
- the dead-actor heartbeat, which also fails on pre-#2353 main 4b981762.

create and adopt pass there, as expected.
