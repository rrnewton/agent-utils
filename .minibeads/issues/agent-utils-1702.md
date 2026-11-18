---
title: 'wrkslots-eden-finish-accounting: Eden slot finish must count Sapling drafts, accept a moved clean checkout, and stop its own daemon unit on remove'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-16T03:15:28.000000000+00:00
updated_at: 2026-11-16T03:15:28.000000000+00:00
---

# Description

Reported by a consuming team's lead on 2026-11-15, from finishing one Eden
(Sapling) agent slot:

1. Draft accounting is blind to Sapling. finish reported "accounting
   landed=0 open=0 unaccounted=0" while the checkout's own `sl journal`
   named 18 drafts (all from three landed diffs, as it turned out). The
   accounting therefore rested entirely on the --validation text, which is
   exactly what accounting exists to check.
2. The Eden finish safety check (_assert_eden_checkout_safe) requires the
   checkout to sit at the revision recorded at provisioning, and nothing
   updates that record. A clean checkout moved to current master is refused
   ("checkout main revision changed"), so the lane has to `sl goto` the old
   base before it can finish.
3. remove was refused because the slot's own per-slot daemon unit
   (edenfs@<escaped state dir>.service, #1423) was still running. The
   operator had to ask whether stopping it by hand is the intended remedy.

# Design

1. Accounting for Eden checkouts enumerates Sapling drafts (draft() and
   any non-public commits visible to the checkout, plus the journal's
   recent heads) and classifies each like Git commits: landed (its change
   is in public history, matched by the commit-message differential
   revision or by patch identity), open (on a remote review), or
   unaccounted. finish refuses on unaccounted drafts unless they are
   salvaged.
2. The safety check proves the checkout is clean and its working copy is
   recoverable; it must not pin the provisioning revision. Accept any
   revision whose drafts are all accounted for, and record the observed
   revision in the finish receipt. Keep refusing a dirty checkout or
   unaccounted drafts.
3. remove of an Eden slot stops and disables that slot's OWN daemon unit
   as part of the Eden remove sequence (after the salvage proof and before
   the unmount), using the unit name derived from the slot's own config
   dir, and verifies the daemon is gone. It never touches the shared
   default daemon or another slot's unit. If the unit cannot be stopped,
   refuse naming the unit and the command to run.

# Acceptance Criteria

- Stubbed-Sapling tests: a slot with landed drafts counts them as landed;
  an unlanded draft is unaccounted and blocks finish; the --validation text
  alone cannot make a draft count as landed.
- A clean checkout moved to a newer public revision finishes; a dirty one
  is refused; the receipt records the observed revision.
- remove stops the slot's own edenfs@ unit, verifies it is gone, and never
  names another slot's unit or the shared daemon; a test pins the derived
  unit name.
- No real Eden clones in tests (stubbed runner), as for #1558.

# Notes

Related: #1554 part B (Eden remove sequence), #1558 (create-eden journal),
#1423 (per-slot daemon unit), #1700, #1701.

## Resolution

Done (mu-wsfix), stubbed runners only (no real Eden):

1. `eden.classify_drafts` enumerates the checkout's `draft()`
   commits (which include the journal's recent heads), reads the
   public history's Differential Revisions and the bookmarked
   drafts, and classifies each draft landed (its Differential
   Revision is in public history), open (bookmarked for review),
   or unaccounted. `_assert_checkout_safe` now runs it for Eden
   checkouts under `accounting=True`, so finish totals for an
   Eden slot rest on `sl` evidence, never on the --validation
   text alone.
2. `_assert_eden_checkout_safe` no longer pins the provisioning
   revision: a clean checkout at any revision is accepted, dirt
   still refuses, and the observed revision is returned on the
   checkout so the finish receipt records it (finish already
   stores the final checkouts).
3. `eden.remove_sequence` stops and disables the slot's OWN
   `edenfs@<escaped state dir>.service` unit (`systemctl --user`,
   name from the one derivation in `eden.unit_name`, which
   `_edenfs_unit_name` now delegates to) after `eden stop`, and
   verifies it is gone; a unit that cannot be stopped refuses
   naming the unit and the exact command. No other slot's unit
   and no shared daemon is ever named.

Tests: `test_classify_drafts_counts_landed_and_unaccounted`,
`test_finish_accounting_counts_eden_drafts`, the moved-checkout
case in `test_eden_checkout_safe_proofs` (now accepted with the
observed head), and unit stop/disable/verify assertions in
`test_remove_sequence_cleans_and_stops` — all in
`py/wrkslots/tests/test_eden.py` with stubbed runners.
