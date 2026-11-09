---
title: 'finish-journal-live-remover: a live removal''s finish journal reads as interrupted, not busy, during out-of-lock deletion'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-09T11:00:09.000000000+00:00
updated_at: 2026-11-09T11:00:09.000000000+00:00
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
