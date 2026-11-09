---
title: 'finish-journal-live-remover: a live removal''s finish journal reads as interrupted, not busy, during out-of-lock deletion'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-09T11:00:09.000000000+00:00
updated_at: 2026-11-09T12:00:00.000000000+00:00
---

# Description

Since #894, an ordinary removal fences and journals under the project-root lock,
deletes the fenced tree with the lock released, and re-takes the lock only to
record completion. The finish journal (ACTIVE.<machine>.journal, kind "finish")
stays outstanding for that whole out-of-lock phase.

The finish journal names its remover only as the string actor "coordinator".
_live_journal_owner (Python cli.py; Rust classify_journals.rs live_journal_owner)
takes the first parseable identity in the order owner, record.owner, actor,
coordinator_lease, and a later live identity does not resurrect a dead first one.
For a finish journal the first identity is therefore record.owner: the agent that
owned the slot being removed. Retirement removes slots whose agent has exited, so
that owner is dead. classify-create-journals then reports a removal that is alive,
seconds from completing, as an interrupted mutation (exit 3), instead of busy
(exit 75, which callers wait out).

Every validate-run entry classification that lands in the out-of-lock window is
refused. That includes merge-queue launches. On 2026-11-09 this ended a
consumer's nightly validation run on another host: finish journal written
06:06:32Z with a dead record.owner, fenced 06:06:43Z, the waiting run refused at
06:07:00Z, the finish completed 06:07:02Z. (The consumer is not named, per
repository policy.)

A proposal and reproduction were written by a consumer-side lane and passed on by
the lead coordinator. Only the shape is reproduced here.

# Design

1. Finish journals written by remove carry the remover's own process identity as
   top-level owner, the same identity object as record.owner (pid, start_ticks,
   boot_id, host_id, cgroup_path).
   - Python: the finish-journal payload / _begin_finish; add "owner" to
     _FINISH_JOURNAL_OPTIONAL.
   - Rust: the journal objects in rs/wrkslotsd/src/remove_write.rs, and the
     finish-journal field list in journal_state.rs.
2. _live_journal_owner needs no change: it already checks top-level owner first,
   in both editions.
3. Anything that resumes a finish journal for another process (recover, a later
   remove resuming a fence) rewrites owner to the resuming process. A crashed
   remover then reads as interrupted, and a resumed one as busy.
4. Legacy finish journals without owner stay accepted by readers and keep
   today's behaviour.
5. Check #914 wrkslots-dead-seal-journal (dead seal plus paired finish journals)
   for overlap, and say how the two compose.

Not proposed:
- making callers wait on every interrupted finish journal (a dead remover is
  indistinguishable without the identity and needs recover);
- holding the lock through deletion again.

The Python edition lands first; mu-rswrk2 (devbig014) ports Rust unless the
Python change needs the Rust writer for the differential to stay exact.

# Acceptance Criteria

- Finish journal, live remover as owner, dead record.owner: classify-create-journals
  exits 75, busy, kind finish.
- The same journal with a dead remover: hard refusal containing "interrupted".
- Legacy finish journal without owner: still parsed, still refused as interrupted
  when record.owner is dead.
- A resumed finish journal names the resuming process.
- #914 overlap stated in Notes.

# Notes

P1: it hits every validate-run caller. Filed by the devbig030 coordinator from
the lead's relay, 2026-11-09.

Overlap with #914 wrkslots-dead-seal-journal (cl-wrkid, 2026-11-09): the two compose. #914 lets a provably dead actor's validate-batch-seal journal and its paired finish journal self-recover. Until now a finish journal had no remover identity to prove dead: its actor is the string "coordinator", and the only identity in it was the removed slot's owner. The new top-level owner is exactly the identity #914 needs: a live owner reads busy (75) and must be waited out, and a dead one lets #914 recover the pair. A legacy finish journal without owner gives #914 nothing to prove dead, so it keeps today's refusal. #914's issue file records the same.

## Resolution

Done in both editions in one landing (cl-wrkid, 2026-11-09). Python: `_finish_journal_payload` writes `owner`, the removing process's identity (pid, start_ticks, boot_id, host_id, cgroup_path); `_recover_finish` rewrites it to the recovering process before resuming, so a resumed journal names its resumer (only when the journal file exists: a journal rebuilt from the event history must not gain a file, which `_prepared_finish_journal` takes as proof that the removal began; `test_finish_recovery_without_a_journal_still_refuses_a_pruned_admin_checkout` guards that); `owner` is in `_FINISH_JOURNAL_OPTIONAL` and validated as an identity (`_finish_journal_owner`). Rust: the remove writer in remove_write.rs writes the same field, and journal_state.rs accepts and validates it. Rust never resumes a finish journal (it leaves that to the Python edition), so it has no resumer to change. `_live_journal_owner` was unchanged in both, as the Design said.

Tests (py/wrkslots/tests/test_remove_write_rust.py): `test_finish_journal_classifies_by_its_remover_in_both_editions` (a killed remover reads interrupted; a live remover with a dead record.owner reads busy, 75, kind finish, in both editions; a legacy journal without owner is read and refused as interrupted, as before); `test_remove_writes_the_same_finish_journal_as_python` (every finish journal each edition records is identical apart from per-process and per-run values; owner keys included); `test_recovery_rewrites_the_finish_journal_owner`; `test_an_older_reader_refuses_only_an_outstanding_owner_journal`.

Upgrade exposure (accepted by the lead, 2026-11-09; measured with a stand-in for an older reader, in the last test, and stated in the USER_GUIDE and the commit message): while a newer pin has a finish journal outstanding, an older pin's audit, recover and classify-create-journals refuse with "finish journal has invalid fields: unknown owner", until the removal completes or, after a crash, a newer recover finishes it (an older recover cannot). status and doctor are unaffected; mutations refuse as they already do while any journal is outstanding; a completed removal leaves nothing an older pin refuses. Not measured: a real older pin. The stand-in removes only the key from the reader's optional list. A tolerant extension slot for journal readers is a separate P3 follow-up, filed by the coordinator.

Unrelated parity gap seen while testing: for an interrupted journal the Rust classify-create-journals exits 1 ("host fault") where Python exits 3. Both name it interrupted. Left as is.

