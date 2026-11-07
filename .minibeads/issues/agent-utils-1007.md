---
title: 'create-journal-rollforward: a scoped create journal one progress step behind its event log blocks every create until recover rolls it forward'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-07T06:20:00.000000000+00:00
updated_at: 2026-11-07T06:20:00.000000000+00:00
---

# Description

Wrkslots writes every scoped operation journal event-first: `_write_journal`
appends an `operation-progress-recorded` event to the append-only log, then
atomically rewrites the journal file. A writer that dies between those two
writes leaves the journal exactly one progress step behind the log. That is a
normal crash window, not corruption.

The strict provenance check `_scoped_operation_journal_completed` requires the
journal bytes to equal the latest progress event. With the journal one step
behind, `_validate_journal_provenance` raises "differs from append-only
progress evidence", so `_load_journal` and `_classify_create_journals` refuse:
`recover --abort-create` cannot start, and every `wrkslots create` and
`classify-create-journals` on that machine shard refuses with "REFUSED: scoped
create journal differs from append-only progress evidence". One interrupted
validate create therefore blocks all new lanes on the shard.

`recover --discard-partial` makes it worse. Its event-temp branch validates a
complete `EVENTS.<machine>/NNNN.json.tmp.*` file (digest, hash chain and
sequence) and promotes it to durable with `os.replace`. Promotion is sound —
the temp passed full validation, so the step it records really completed — but
nothing then rolls the journal forward, so one recover run manufactures the
exact lag the next recover refuses. The journal temp leftovers, by contrast,
correctly keep the last durable state.

Observed on a consumer host: a validate create was cancelled during a
disk-floor event, leaving an unrenamed event temp and a `CREATE.*.journal`
whose bytes equal progress event N-1 (hook running) while the promoted event
N says the hook is done. After promotion, `recover --abort-create` refused
(rc 3) and every create on the shard refused.

# Design

Events are authoritative. Add a supported roll-forward in `recover` only:

- When a scoped operation journal (create and finish share the window) equals
  the progress immediately before the latest progress event on its path —
  same operation identity, same episode, exactly one step behind, latest
  episode still open — `recover` atomically rewrites the journal from that
  latest event's journal payload and prints what it did.
- The roll-forward runs under the mutation lock immediately after partial
  updates are recovered and before any other refusal, including the
  ownerless-validation coordinator-authorization refusal, and it is persisted
  even when the rest of that recover run then refuses. A plain
  `recover --coordinator-pid P` on an already-promoted lag (no temp file)
  must therefore repair the journal.
- When the temp-event promotion in the same run moves the log ahead of a
  journal, the same roll-forward repairs the journal in that run. Promotion
  is kept: discarding a validated event would lose a completed hook step
  and could re-run it unsafely.
- Anything else (more than one step behind, identity change, cross-episode
  reuse) still refuses, and `_scoped_operation_journal_completed` stays
  byte-strict for every other caller.

The Rust `wrkslotsd` edition reimplements the same event-first protocol and
the same strict check, but its recover path deliberately leaves partial-update
recovery with the Python edition (it ignores `discard_partial`), so this fix
lands in Python and the Rust divergence is documented rather than duplicated.

# Acceptance

- Exact sequence: crash after the event temp file is written, before the
  rename and the journal rewrite; then `recover --discard-partial`; then
  `recover --abort-create`; then a new create succeeds.
- Journal one step behind with no temp file: plain `recover` rolls it
  forward even when the run then refuses the ownerless validation cleanup;
  afterwards `recover --abort-create --slot X` succeeds and a new create
  succeeds.
- Refusals preserved: two steps behind and wrong identity still refuse with
  "differs from append-only progress evidence" and leave the journal
  untouched.
