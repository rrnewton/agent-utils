---
title: 'wrkslots-finish-commit-accounting: finish accounts for every branch commit'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-12T12:15:00.000000000+00:00
updated_at: 2026-11-12T12:15:00.000000000+00:00
---

# Description

[watermelon-full-preview] From a downstream team's lead, after the owner (2026-11-12) was angry
that slot cleanup is not a predictable part of every task's wind-down: there are 98 dead slots
on one host and 27 on another. `wrkslots finish` proved only that a checkout HEAD was
published and an ancestor of the landed ref, so work landed by replay (cherry-pick, rebase,
squash) read as unlanded, published-but-unmerged work could not finish at all, and commits on
nested worktrees inside a slot were invisible until removal tripped over them.

# Design

- Finish does the commit accounting itself. For every commit on the slot's branch(es), and in
  its nested linked worktrees when Git lists them for a recorded repository inside the slot,
  not accounted by the landed ref: classify by patch-id against the configured landed ref
  (landed, including a replay under another SHA, via Git's own patch-id equivalence in
  `git cherry`), and against published remote-tracking refs (open, using the same containment
  `wrkslots unpushed` computes), else unaccounted. Candidates are the branch commits since the
  recorded start point (merge base for nested tips).
- Finish prints `landed`/`open`/`unaccounted` counts with the unaccounted SHAs and subjects,
  and refuses while anything is unaccounted. `--discard-unaccounted` bundles the discarded
  history into the local salvage archive (`wrkslots-salvage/finish-<slot>-<checkout>-<gen>.bundle`,
  verified before recording), records the discarded SHAs in the handoff
  (`discarded_unaccounted`) and the counts in the `handoff-recorded` event evidence.
- Removal rechecks with the same accounting: handoff-discarded SHAs stay discarded, every
  other branch commit must still be landed or open, and checkout identity is the heads
  (containing remote refs legitimately move as work lands and branches are deleted).
- Both editions: Python `py/wrkslots/cli.py` and the Rust port `rs/wrkslotsd`
  (`finish_write.rs`, `--discard-unaccounted` flag, handoff field in both state readers).
  No second engine: open reuses `unpushed`'s containment primitives verbatim.
- Coordination with #1302 wrkslots-ignored-evidence-salvage: that issue owns ignored-file
  handling at removal. Finish keeps refusing dirty/ignored checkouts exactly as before;
  commit accounting is orthogonal and `--discard-unaccounted` never discards files.

# Acceptance Criteria

- Fixture-repo tests (no network) cover: landed by exact SHA, landed by replay (same
  patch-id, different SHA), open on a published ref, unaccounted (finish refuses, naming
  SHA and subject), `--discard-unaccounted` (finishes, records the SHA, bundle verifies),
  and an unaccounted commit in a nested worktree (refuses).
- The same five cases run against both editions in the finish differential.
- Existing finish lifecycle tests pass under the new contract (a pushed-but-unmerged
  branch now finishes as open; forged local tracking refs still refuse).

# Notes

[watermelon-full-preview] Number reserved by coordb. No pre-existing finish-accounting
issue existed in `.minibeads/issues` when filed (title search, 2026-11-12).
