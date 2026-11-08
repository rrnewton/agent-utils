---
title: 'standalone-journal-busy: a standalone recovery journal mismatch during an in-flight slot operation refuses hard instead of reporting retryable busy'
status: closed
priority: 1
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-08T00:20:00.000000000+00:00
updated_at: 2026-11-08T13:20:05.000000000+00:00
closed_at: 2026-11-08T13:20:05.000000000+00:00
---

# Description

On 2026-11-07 at 7:04 PM ET, a consumer's merge queue (tool root at agent-utils a7bd1a1c, which already has 5e1d1c99 "report a live non-create journal as retryable busy") asked wrkslots to classify create journals before a validate run and got:

    wrkslots could not classify create journals: REFUSED: standalone recovery journal differs from append-only operation evidence

after 11.656 s (11.655 s classifying, 0.000 s waiting). The job ended COULD_NOT_RUN and was requeued. Moments later worktrees/ACTIVE.<machine>.journal was gone, and classify-create-journals reported NONBLOCKING, journals=0. So the refusal was a race with another slot operation in flight. The standalone branch of the journal provenance check (py/wrkslots/cli.py _validate_journal_provenance, the "standalone recovery journal differs from append-only operation evidence" refusal near line 10198) still refuses hard mid-flight, where the 5e1d1c99 path reports a retryable busy. It is a sibling of #1007 create-journal-rollforward.

The Rust edition has the same refusal in rs/wrkslotsd/src/journal_state.rs (near line 3290).

# Design

- When the standalone recovery journal differs from the append-only evidence AND the journal's operation is still in flight (its writer lock is held or its owner pid is alive with its task scope live, as in 5e1d1c99 and 5ba9c3ec), report a retryable busy, or wait and re-classify within the existing bound. Do not refuse.
- Keep the hard refusal when no operation is in flight: a mismatch with nothing running is real corruption.
- Same behaviour in both editions (py/wrkslots and rs/wrkslotsd journal_state.rs), with a parity test.

# Acceptance Criteria

- A test races a slot create against classify-create-journals and never sees the hard refusal: it gets busy, or a classification after the operation finishes.
- A test keeps the hard refusal for a mismatched standalone journal with no live operation.
- Both editions agree (parity test).

# Notes

Order for mu-wrkslots: after #993, ahead of #1029 clean-caches-skip-running-gate.

## Concurrent operation (reported by the consumer, 2026-11-07)

- The concurrent mutation was a wrkslots reclaim/finish of one of the consumer's slots, EVENTS 43965-43977: prepared 00:02:28Z, fenced 00:02:44-00:03:09Z, removed 00:03:11Z, completed 00:03:21Z (2026-11-08 UTC). The refusal window overlaps the prepared-to-fenced span.
- The standalone journal compared mid-flight was worktrees/ACTIVE.<machine>.journal (cli.py _journal_path), live for that reclaim. By 00:04:30Z it was absent and classify said NONBLOCKING, journals=0.
- Repro suggestion: run a slot reclaim (prepare -> fence -> remove) concurrently with classify-create-journals.

## Resolution

[watermelon-full-preview mu-salvage] Landed on github/main as 953bbb4c. Classify in both editions now reports a standalone journal mismatch as retryable busy only with real in-flight proof: the registry writer lock is held (read from /proc/locks, with a descriptor scan fallback for boxed steps), or a task scope the journal carries still verifies for its recorded owner. A merely live slot owner or coordinator no longer counts, so the hard refusal stands when nothing is in flight, including for a tampered legacy create journal. Parity tests hold the writer lock over a mismatched standalone journal (busy, exit 75, both editions) and keep the hard refusal with no live operation. Gate on the final commit: 62/63 selected steps in the full gate, with python.wrkslots.shard-6 passing in its exclusive rerun. Unverified: no live reclaim was raced against classify on a production registry; the race is covered by the lock-held fixture in both editions.
