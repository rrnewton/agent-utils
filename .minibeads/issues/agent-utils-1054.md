---
title: 'merge-queue-sync-exact: label-driven sync cannot mark an item exact'
status: closed
priority: 2
issue_type: feature
created_at: 2026-11-08T10:36:49.000000000+00:00
updated_at: 2026-11-09T01:14:42.000000000+00:00
---

# Description

`submit --exact` (#922 merge-queue-exact-merge-head) lands a
submitted merge commit exactly — both parents intact, fast-forward
only, never rewritten — but only the CLI submit path can set the
flag. The daemon's label sync constructs submissions with
`exact: false` unconditionally (`rs/merge-queue/src/sync.rs`), so
a queue that ingests by label cannot land exact merges at all.

A consuming project wants frequent, automatic, high-priority
landing of two-parent catch-up merges (an upstream tip merged
into its main by automation). Driving that through the sync label
it already uses is the natural shape; today the automation must
instead shell out to `merge-queue submit --exact --prioritized`
per merge, beside the daemon.

# Design

- A config mapping beside the existing priority label (e.g. an
  `exact_label` in the same config block): a synced pull request
  carrying that label is submitted with `exact` set, persisted
  on the item exactly as `submit --exact` persists it (intake
  records included, stays set on resubmission). Unlabelled
  synced changes are unaffected.
- The flag follows the label the way `prioritized` follows the
  priority label at sync time; `list`/`status` output shows an
  exact item as exact, however it was submitted.
- No new landing semantics: exact items keep every current rule
  (singleton batch, `lands_exactly` fast-forward test,
  `not-fast-forward` ejection when the base moved, no rewrite by
  pins/recount/regeneration). Priority still composes through the
  existing priority label / `--prioritized`, and `--after`
  composition is unchanged.
- The user guide's sync and exact sections cross-reference: the
  label is how a daemon-only queue requests exact landing, and
  the first parent of the merge must be the base at landing time.

# Acceptance Criteria

- A label-synced two-parent merge made on the current base lands
  exactly (both parents intact, fast-forward) under
  `combine=rebase`, in a batch of its own.
- The same merge without the label keeps today's behaviour (the
  `--no-merges` replay under rebase combine).
- A synced exact item whose base moved first is ejected
  `not-fast-forward` with the rebuild-and-resubmit reason, and a
  later sync of the rebuilt merge lands it.
- Existing sync, priority-label and exact tests keep passing.

# Notes

Gap (1) from the #922 follow-up review. Gap (2) — the
conflicts-with-base probe merge-treeing exact items — is folded
into the rebase-aware conflict probe issue, which lands first.
The CLI path (`submit --exact --prioritized`) already covers
automation that submits explicitly; this issue is only about the
label-driven daemon path.

## Resolution

Landed on main as 22b65f014cb49413916e85581e3f6f2ef1e4426d (gated commit 3b79d2ca, in the same push as #1094's 3592b245).

`sync` can now submit a change exact: `init --exact-label LABEL`
names a label (compared without regard to case; empty or `-` is
off, stored as `priority.exact_label`) that hosting sync honours
like the priority label, and a ready-file line can use the bare
word `exact`. Reconcile resubmits when a change's exactness
differs from its entry's. Label-driven catch-up merges can now
land by fast-forward automatically instead of by hand with
`submit --exact`. With #1094 the label is followed both ways:
removing it clears exact on the next reconcile, unless a peer
requirement pins that head. Covered by cli_sync tests (ready-file
word, configured label sets exact and resubmits on change, label
removal clears). Gate: full shared phase on the gated stack (dagrun 238 passed / 1 failed; rust.merge-queue.test clean at 585 passed / 0 failed across 51 binaries, wall 534.312s at user 519.555 + sys 776.082 CPU-s). The one failure was untouched timeline.browser.functional, refused in 1s because another lane's server held its health port, and it passed solo in an exclusive rerun (38 passed in 49s). The gate-exclusive cross differentials were stopped under the coordinator waiver for this merge-queue-only diff. Main had moved one overlapping merge-queue change set (plan-intake naming, run.rs/USER_GUIDE/cli/state context) since the gated base; the replay is patch-identical, and the combined tree passed the whole merge-queue crate (591 / 0) with fmt, clippy -D warnings and the client-names check clean; its merge-queue paths are byte-identical to the landed tree.
