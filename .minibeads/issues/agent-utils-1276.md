---
title: 'adopt-lock-narrow: adopt and create validate the whole registry while holding the exclusive project lock'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- lock-scope
created_at: 2026-11-11T02:36:47+00:00
updated_at: 2026-11-11T06:12:09+00:00
---

# Description

[opus 5.5] A consumer sampled its configuration lock on a shared host on 2026-11-10: `/proc/locks` every 2 s for 20 minutes. The lock is an exclusive flock on the project root (`_locked_config`, entered by `_mutation_locks`). The consumer's run-population lock (ci-hub `run_population_lock`) takes the same flock, so every long wrkslots hold also blocks validation setup, which was refused with "configuration lock is busy".

The longest holders were plain `wrkslots adopt` runs: 430 s, 236 s and 56 s. An agent `wrkslots create` held it for 57 s. Of the 382 samples in which a wrkslots command held the lock, 348 were adopts and 34 were creates. The registry then had 185 active slots and 78,892 event files.

Cause: `_cmd_adopt` runs everything inside `with _mutation_locks(...)`. That covers `_refuse_partial_state`, `_assert_no_journal`, `_validate_global_state` (every shard's event history and the archive), `_assert_command_registry_storage` (every slot's storage), `_global_rows`, and then, after the write, a second `_validate_global_state`. `_cmd_create` has the same shape, plus `_classify_create_journals` twice, a registry-wide slot and agent check, and a storage report over every row. So a mutation of one record validates the whole fleet while it holds the exclusive lock.

The same family: #1194 wrkslots-lock-scope (salvage and census before the lock), #1172 (readers verify history before the shared lock), #1247 wrkslots-status-lock.

# Design

Implemented on branch `dh/adopt-lock-narrow`, for review before landing.

- `_run_prevalidated_mutation(config, wait, command, prepare, commit)`. Each round takes the registry identity, then runs `prepare` (the fleet-wide validation) without holding any lock. Then it takes the exclusive mutation locks and compares the identity. Only an unchanged identity runs `commit` under the locks. A changed identity releases the locks and starts a new round. After `REGISTRY_VALIDATION_ATTEMPTS` (5) overtaken rounds, the command refuses as busy and writes nothing. That refusal is a `_LockBusy` ("registry is busy: ... as when the configuration lock is busy"), so callers that retry a busy lock retry it. `--wait-lock` bounds the lock waits of all rounds together.
- `_registry_identity`: one lstat per registry file, never a content read. It covers the `ACTIVE.*`, `ARCHIVED.*`, `CREATE.*`, `FINISH.*`, `HANDOFF.*`, `HOLD.*` and `VALIDATE-BATCH-SEAL.*` names and any top-level `*.journal` (partial-update leftovers included), every file of every `EVENTS.*` log, the journals in the `quarantine` directory, and every slot-image record (`slot-images/TYPE/SLOT/IMAGE.json`, never the image beside it); lock files are excluded. Those are the files `_registry_read_identity` covers for readers, and the lock-free validation reads them. The quarantine journals and image records were added after review (F1): before that, a quarantine-only change between the validation and the compare under the lock went unnoticed. It is an allowlist on purpose, because the control directory can also hold non-registry storage, such as a project worktrees image hundreds of GiB in size whose times change with every slot write. Every wrkslots mutation appends an event, replaces a shard, or writes or removes a journal, and an atomic replace brings a new inode. So an equal identity proves that no wrkslots writer committed anything in between.
- Under the lock, the command re-checks only what no registry identity covers, for the record it touches. adopt runs `_assert_target_record_storage_consistent` for its slot. create keeps its plan, its target-collision check and its process and disk checks there, as before. `_refuse_partial_state` stays under the lock, because a temporary file means an interrupted writer only while no writer can hold the lock.
- Nothing under the lock loads or lists an event log. `_VerifiedLocalLog`: with the identity unchanged, this machine's event files are exactly the chain the lock-free validation verified and folded, so the command takes this machine's ACTIVE and ARCHIVED state from that validation, and the tip from the identity's event names (their count is the sequence) plus the last event file, which is the only earlier event read again. Its writer appends after that tip. `_write_active_state` takes that verified state as `previous=` instead of loading it. `_write_active_state`, `_write_journal` and `_clear_journal` skip `_ensure_event_log`, a listing of the whole log, when the writer is positioned after an existing event. create writes its ACTIVE and ARCHIVED views from the verified states, which is what `_ensure_state_shard` does for a log with events.
- Post-write check: `_assert_registry_change_limited_to_slot`. When only this machine's shards and event log differ from the verified identity, plus the slot's own image record for an image-backed create, `_appended_events_limited_to_slot` reads just the events appended after the verified tip. It accepts only contiguous new files chained from the tip, of the kinds adopt and create append, each naming only the slot, and each active-state event advancing the verified revision by one from the slot's own record to a record of that slot whose paths check out. Any other shape re-derives this machine's rows by replaying its log, and any other registry difference gets the full re-validation, as before.
- An outstanding journal (an interrupted mutation's, a live writer's, or a validation batch seal) makes that round defer to the original locked path, because journal recovery and create-journal classification need the lock. A live writer's journal disappears when it finishes, which changes the identity, so the next round validates lock-free again.
- A refusal raised lock-free is reported only from a round in which the registry held still. A round that another writer overtook judged a state that no longer exists, so the next round judges the current one.

# Acceptance Criteria

- adopt and create run no fleet validation, fleet storage scan, journal scan or journal classification while holding the exclusive lock, counted rather than timed (`py/wrkslots/tests/test_prevalidated_mutation.py`).
- A writer that commits during the lock-free validation (a real `hold` in another process, which can only take the lock because the validation holds none) sends the command back to validate. Neither validation holds the lock. Tested for both commands.
- When every round is overtaken, the command refuses as busy after the bounded number of rounds and writes nothing. Tested for both commands.
- Nothing under the lock loads or lists an event log. The event files read under it are the verified tip plus the events the command wrote: 2 for adopt, 1 plus its events for create. Counted for both commands.
- The post-write check reads only the appended run when it has the expected shape, and replays the log otherwise (tested).
- A quarantine-only change between the lock-free validation and the compare under the lock sends the command back to validate. Tested for both commands; the test fails without the identity change (review F1).
- An outstanding journal keeps the original locked path, with the same refusal and classification.
- The existing lifecycle, lock-scope, event-memo and Rust differential suites pass.

# Notes

**Exclusive windows and lock-held counts.** Measured on 2026-11-10 on a disposable copy of the measured registry: 79,044 event files, 193 active rows, 3,416 archived records. Each version ran create of an unowned slot, then adopt of it. Each command was a fresh `cli.main` call with a fresh event memo, and each version ran on its own fresh copy of the registry. The host has 316 CPUs; loads are 1-minute averages before and after each version's run. Round 3 ran the four versions back to back, while other work moved the host's load between 161 and 256. Round 1 ran at 33 to 156. The counts are from round 3. They are the same in every round, give or take two event reads from the memo's two-second stability window. The windows vary with load.

| held under the exclusive lock | pre-#1247 b295042ac0 | main 8b1e82a624 (#1247 cap) | 963c3e9b alone (#1261) | this branch on 963c3e9b |
|---|---|---|---|---|
| **adopt**: exclusive window, round 3 (back to back) | 28.8 s (load 239-256) | 127.0 s (load 256-161) | 25.5 s (load 208-239) | 3.5 s (load 193-208) |
| adopt: exclusive window, round 1 | 26.4 s (load 97-156) | 94.7 s (load 33-97) | 16.6 s (load 46-38) | 1.2 s (load 38-33) |
| adopt: event files read and verified from disk | 79,051 | 711,443 | 79,051 | 2 |
| adopt: whole-log loads / log listings | 9 / 10 | 9 / 10 | 9 / 10 | 0 / 0 |
| adopt: fleet validations | 2 | 2 | 2 | 0 |
| adopt: storage rows scanned | 194 | 194 | 194 | 1 |
| adopt: git subprocesses | 7 | 7 | 7 | 8 |
| **create**: exclusive window, round 3 (back to back) | 47.8 s (load 239-256) | 275.0 s (load 256-161) | 48.9 s (load 208-239) | 11.7 s (load 193-208) |
| create: exclusive window, round 1 | 39.9 s (load 97-156) | 152.5 s (load 33-97) | 25.0 s (load 46-38) | 2.6 s (load 38-33) |
| create: event files read and verified from disk | 79,051 | 1,185,678 | 79,051 | 6 |
| create: whole-log loads / log listings | 15 / 20 | 15 / 20 | 15 / 20 | 0 / 0 |
| create: fleet validations | 2 | 2 | 2 | 0 |
| create: storage rows scanned | 193 | 193 | 193 | 0 |
| create: git subprocesses | 40 | 40 | 40 | 36 |

The lock-free validation still takes most of each command's wall time on that registry: adopt took 14.7 s of wall time at load 33 to 38 and 21.1 s at load 193 to 208. No lock is held during it.

The #1247 retention cap made every load under the lock a cold re-read (the main column). #1261 scoped the cap to read-only scans (the 963c3e9b column), which brings the windows back to their pre-#1247 size, but that version still loads the whole log 9 times under the lock for adopt and 15 times for create. This branch loads it 0 times.

**What this branch still does under the lock.** Time under the lock per function, at load 33 to 38 (round 1) and 193 to 208 (round 3):
- Two registry identity scans, one lstat per registry file (every event file plus the shards and journals): 0.45 s and 0.7 to 0.8 s together. About half of that is the post-write check's scan.
- adopt: the touched slot's own storage check (`_assert_target_record_storage_consistent`, Git worktree probes): 0.46 s and 2.3 s.
- create: its plan, including the remote fetch (`_create_plan`, here against a local bare remote): 0.73 s and 2.0 s. Then `git worktree add`, the checkout verification and the target-collision check, and the writes that end in fsync: the journal events, the ACTIVE and ARCHIVED views (the ARCHIVED view is one 3,416-record file), and the journal removal (`_clear_journal` took 2.3 s at load 207). The whole create window was 2.6 s at load 33 and 11.7 s at load 207, and none of it was event-log work.
- create's post-provision hooks. This registry copy has none; one consumer's hooks include a recursive submodule update and a generator bounded at 300 s.
- Other single-record mutations with the same validate-under-lock shape, not converted: `_cmd_heartbeat`, `_cmd_hold`, `_cmd_unhold`, `_cmd_release`, `_cmd_transfer_owner`, `_cmd_supersede_handoff`, `_cmd_recover_unbound_owner`, `_cmd_register`, `_cmd_import_existing`, `_cmd_finish`, `_cmd_write_handoff`. None of them appeared in the 20-minute sample. `_run_prevalidated_mutation` and `_VerifiedLocalLog` are written so they can be converted one at a time.

**Why 5 rounds.** The measured host's event log shows 1,723 distinct mutation bursts in 24 hours. A 20 s window sees at least one of them about 15% of the time, and a 60 s window about 27%.

**Found while reading** (reported to the #1247 wrkslots-status-lock owner and fixed there): `_registry_read_identity`, used by `status` and `doctor`, hashed the contents of every non-lock file in the control directory with `read_bytes()`. That included a worktrees image file, 687,194,767,360 bytes apparent on the measured host. #1247 now hashes only registry files.

**Rust edition** (not changed here; parity belongs to its owner, as in #1194): `rs/wrkslotsd/src/adopt_write.rs` and `create_write.rs` have the same validate-under-lock shape. Both take `acquire_mutation_locks` or the config lock and then run `refuse_partial_state`, `parse_journals` and `validate_global` while holding it. Both writers are fixture-only until the edition cutover (`ensure_states::require_fixture`), so no live registry sees them yet.
