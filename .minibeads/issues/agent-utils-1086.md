---
title: 'validate-run-authority-budget: a donor entry-authority refusal at 9.7 s on a loaded host — budget too tight or real authority gap?'
status: open
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-08T19:47:11.000000000+00:00
updated_at: 2026-11-08T19:47:11.000000000+00:00
---

# Description

On 2026-11-08 at 18:55:37Z the consumer's merge queue reported a donor
refusal from devbig030, job v4 (the consumer's PR 5302):

> validate-run: REFUSED: prospective validation slot/path authority could
> not be proved after 9.720s ... wrkslots

The trailing detail after the colon was truncated in the report that reached
us, and the exact incident line could not be found afterwards in the
consumer's validate logs, validate-ledger spool, or run handles on this host
(an admission that never ran leaves no run handle). The host was heavily
loaded at the time (load average ~150 on 316 cores), with several shared
gate tickets running.

## Findings (read-only investigation, 2026-11-08)

- The refusal is raised by the consumer's validate-run entry admission
  (`require_validate_entry_cleanup`): it holds the run-writer generation and
  run-population registry locks, then classifies interrupted create journals
  by shelling out to wrkslots `classify-create-journals`, and inspects
  prospective run handles. Any `OSError`/`RuntimeError`/`ValueError` in that
  span is re-raised as this one refusal, with the elapsed split
  "(<classifying>s classifying, <waited>s waiting for registry locks)".
- The classification budget is 30 s + 4 ms per wrkslots event in the event
  chain, capped at 300 s, sized by counting the wrkslots events; the
  registry-lock wait bound is 900 s. The entry classification drives
  wrkslots subprocesses (`classify-create-journals`) through a budget-bound
  runner, so host load stretches every subprocess inside the same fixed
  budget — the same failure shape as #954's remove budget under load.
- The 9.720 s total is far below both nominal bounds, so the refusal was
  not a budget expiry on its face: either a sub-classification command hit
  its remaining-budget timeout early (because counting/classifying events
  had already consumed the budget under load), or a real authority conflict
  (overlapping slot, run handle, or interrupted create journal) was found.
  Without the inner error text these cannot be distinguished yet.
- Recording: the consumer's merge-queue adapter maps exactly this refusal
  (by its remedy line) to `MQ_COULD_NOT_RUN_SCOPE: host validation entry
  authority could not be proved` with exit 75, and the merge queue
  (validate.rs `COULD_NOT_RUN_SCOPE_MARKER`) treats a host-scope refusal as
  the host's fault: no strike against the change, job requeued, donor
  paused and probed. So provided the adapter saw the standard refusal +
  remedy lines, this was recorded as host-scope could-not-run, never as a
  red against the change. Confirming the actual receipt line for this job
  is still open (see Acceptance Criteria).

# Design

- First reproduce the evidence: get the full refusal line (inner error and
  the classifying/lock-wait split) for a refusal of this shape, and the
  donor's recorded scope line for the job, before changing any bound.
- Decide budget vs gap from that evidence: if classification subprocesses
  are being starved by host load inside a fixed budget, size or pace the
  entry classification the way #954 sized the remove item budget (or make
  the budget CPU-aware), and keep a named refusal for a real overlap.
- Keep the host-scope recording contract intact: an entry-authority
  failure that says nothing about the change stays
  `MQ_COULD_NOT_RUN_SCOPE: host ...` with exit 75, never a red.

# Acceptance Criteria

- The donor's recorded scope for the 2026-11-08 18:55:37Z job is quoted
  (host-scope could-not-run, or a correction is filed if it was recorded
  any other way).
- A loaded-host reproduction (or the recovered incident line) shows which
  phase consumed the 9.7 s: registry-lock wait, create-journal
  classification (wrkslots), or run-handle inspection.
- Verdict recorded here: budget too tight under load, or a real authority
  gap, with the evidence line that decides it. No fix is started until the
  lead approves the verdict.

# Notes

Reported 2026-11-08 by the lead coordinator from the consumer's merge
queue. The consumer is not named, per repository policy. Investigation was
read-only on the consumer side. Do not start a fix until the lead approves
the verdict.

## 2026-11-08 redirect (a): second condition named

Verdict: a real authority gap, not a budget expiry. The lead's queue record
gives the full line: 9.720 s classifying, 0.000 s waiting for registry
locks, ending in a failed `classify-create-journals`. The host-scope
recording was correct (exit 75, requeued, no red).

Mechanism, reproduced: `_classify_create_journals` scans every outstanding
journal on the host (`_outstanding_journals`) with no per-journal
isolation, so the first per-journal `Refusal`/`StateError` aborts the
whole classification for every unrelated requested slot. A lone orphan
CREATE journal with dead owners is not enough: it classifies
`dead-incomplete-create` and classification returns 0 (reproduced in both
editions). A second condition on that journal is required. All three
candidate conditions were fixture-reproduced against an unrelated
requested slot, each refusing host-wide:

1. archive-claim conflict: `create journal for slot 'slot-b' conflicts
   with an archive` (`StateError` from `_create_journal_items`);
2. ACTIVE-row claim: `create journal for slot 'slot-b' has already
   reached ACTIVE publication` (`StateError` from `_create_journal_items`);
3. indeterminate owner: `create journal for slot 'slot-b' process state
   is indeterminate: owner=...; coordinator=...` (`Refusal` from
   `_classify_create_journal`).

Named second condition: the archive-claim conflict. It is the condition
consistent with the recorded facts — a dead-owner orphan that persisted
from 18:55:37Z until it was cleared by removing the journal and its two
events by hand. Indeterminate liveness is a transient read condition:
it neither persists for hours nor is it cured by deletion. An ACTIVE-row
claim would have left a live registry row claiming the slot, not a mere
orphaned journal. The journal bytes were not captured before the
hand-removal, so this names the condition by elimination from the
recorded facts; after (b) below, every such refusal names its journal
and inner site directly, and no future incident needs elimination.

Reproduction kept as tests:
test_orphan_create_with_archive_claim_refuses_classification_host_wide
in `py/wrkslots/tests/test_lifecycle.py` and
test_orphan_archive_claim_refusal_names_journal_in_both_editions
in `py/wrkslots/tests/test_classify_journals_rust.py`.

## 2026-11-08 (b) delivered: the refusal names its journal

`_classify_create_journals` now wraps every per-journal refusal (except
the retryable live-owner `_JournalBusy`, exit 75, unchanged) with
`_journal_classification_refusal`, preserving the `Refusal`/`StateError`
type and the inner reason; the Rust reader mirrors it with
`journal_refusal` in `rs/wrkslotsd/src/classify_journals.rs`, including
the standalone-mismatch pre-scan and the duplicate-claim checks. The new
refusal text (Python, exit 3; Rust keeps its established exit 1 for
`ClassifyError::Other`):

> REFUSED: host fault: journal /abs/path/CREATE.6.node-a.6.slot-b.journal
> (slot 'slot-b', agent 'agent-new', age=0s) blocks create-journal
> classification: create journal for slot 'slot-b' conflicts with an
> archive. remedy: recover or discard the named journal with 'wrkslots
> recover', then rerun classify-create-journals

So the refusal gives the absolute journal path, slot, agent, and age
(journal mtime), says it is a host fault, keeps the inner site text, and
gives the remedy. No budget or retry bounds changed. This issue stays
open: (c) below is a proposal only.

## (c) Safe-quarantine proposal — not implemented, awaiting lead approval

Quarantine a stranded CREATE journal only when every one of these is
proved: owner and coordinator are both proven dead via `_process_state`
(never indeterminate or disagreeing); the journal's file identity is
stable across two reads; its mtime is older than the longest legitimate
create bound; and no checkout claim overlaps any live registry row —
that is, precisely the `dead-incomplete-create` classification.

Action: move, never delete, the journal into the journal directory's own
`quarantine/` subdirectory, named by timestamp plus the original
filename; append a tombstone operation-log event recording the move and
the journal content hashes; retain quarantined journals under a stated
retention before any pruning. It must never auto-quarantine
indeterminate or dead-disagreeing owners, never quarantine a journal
whose claims overlap a live row, never act under the registry lock
mid-mutation, and never double-act with a concurrent manual
`wrkslots recover`. Reversible by moving the file back verbatim.
Implement only once the lead approves.

## (c) Approved 2026-11-08: conditions recorded before implementation

The lead approves the quarantine in principle, sequenced after #1098
wrkslots-classify-batch-seals, with four conditions on top of the
proposal above:

1. Never automatic. `classify-create-journals` stays read-only.
   Quarantine is an explicit operator command: dry run by default,
   `--yes` to act, `--coordinator-authorized` (like #1089's
   release-frozen-incomplete).
2. Recover first. A `dead-incomplete-create` journal is normally
   resolved by `wrkslots recover`, which rolls the partial create back.
   Quarantine is only for a journal recover itself refuses; the command
   says so and points at recover when recover would work.
3. Moving the journal must not orphan what it describes. The tombstone
   (and the dry run) lists every partial artifact the journal names
   (worktree path, image file, branch, registry claims) and whether each
   exists, so it can be cleaned with supported commands afterwards.
4. Visible afterwards: doctor and audit's summary report quarantined
   journals with age and slot. A `quarantine/` subdirectory is fine.

The proposal's own conditions stand: complete dead-owner proof, stable
identity, age bound, no overlap with a live row, never under the
registry lock mid-mutation, never alongside a concurrent recover,
reversible by moving back verbatim. Both editions, same differential
pattern.

Lead's answers to the plan's design questions:

1. Target by slot. The dry run prints the exact journal path and its
   sha256, and `--yes` must be given those same values; refuse if the
   journal changed in between.
2. Age bound = max(24 h, the longest legitimate create bound wrkslots
   knows about), stated in the refusal text when a journal is too young.
3. No restore command in v1, but "reversible" must stay true. The
   tombstone carries everything a hand restore needs: original path,
   the bytes or a pointer to the quarantined file, and the pending
   entry it closed. The user guide documents the hand-restore steps,
   and one test follows exactly those steps and shows classify back in
   its prior state. If that test is awkward to write, that is the
   signal restore needs a command after all; say so.
4. Report-only retention in v1, no pruning.
5. Close the EVENTS-derived pending entry in the same journaled step
   as the move, so a crash between the two cannot leave classify
   refusing on "missing while append-only history still names it".

Sequencing: survey note Mon 2026-11-09 noon ET, implementation PUSH
Tue 2026-11-10 6 PM ET. This issue stays open for (c).
