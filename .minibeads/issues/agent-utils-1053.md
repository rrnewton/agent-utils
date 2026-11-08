---
title: 'merge-queue-rebase-conflict-probe: the conflicts-with-base probe merge-trees even when the queue combines by rebase'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-08T10:36:49.000000000+00:00
updated_at: 2026-11-08T13:10:18.000000000+00:00
---

# Description

The planner's conflicts-with-base probe (`probe_impl` in
`rs/merge-queue/src/plan.rs`) decides every head with a plain
`git merge-tree` of base and head, whatever the queue's combine
mode is. A consuming project's queue combines by rebase
(`"combine": "rebase"`), and the two methods disagree for a branch
stacked on a change that has since landed *as a rebased commit*:
merge-tree reports textual conflicts (in the reported instance, in
two files the landed change also touched), while a rebase replay
is clean — the already-landed commit drops out by patch-id and the
remaining commits apply. The queue item is held
`conflicts-with-base` with advice to rebase and resubmit, which is
exactly what the queue itself would have done successfully.

Exact items (`submit --exact`, #922 merge-queue-exact-merge-head)
have the same mismatch one step later: their landing test is
`lands_exactly` (first parent IS the base, or a linear
fast-forward), never a merge, but the probe still merge-trees
them. A merge made on the current base probes clean, yet once the
base moves the item can surface as `conflicts-with-base` instead
of the clear `not-fast-forward` ejection the build path already
gives it ("rebuild it on the new base, revalidate, and resubmit").

# Design

- Under `combine=rebase`, the base probe replays the way the
  combine step does: the same per-commit replay as
  `Git::combine_with` in rebase mode (own commits in order,
  cherry-pick/merge-tree primitives, regenerable-path handling
  identical), reporting a conflict only when that replay
  conflicts. Commits whose patch is already on the base drop out,
  as a rebase would drop them. Reuse the existing rebase-combine
  code path rather than forking its semantics a second time.
- Under `combine=merge` the probe keeps today's plain merge-tree.
- For exact items the probe tests what landing tests
  (`lands_exactly`: first parent IS the base, or linear
  fast-forward), not merge-tree. An exact item that no longer
  lands exactly is not held `conflicts-with-base`; it flows to
  build, which ejects it `not-fast-forward` with the resubmit
  message. An exact item that does land exactly is never held by
  a merge-tree verdict.
- Pairwise probing is unchanged (stacked pairs are already
  skipped by ancestry). The probe cache key must keep separating
  the verdicts by combine mode (the config hash already carries
  it) and by exactness: the same (base, head) pair can now have
  different verdicts for an exact and a non-exact item.
- Thread the new inputs through `ProbeOptions` from the ops
  callers, which have the items (and their `exact` flags) and the
  configured combine mode.

# Acceptance Criteria

- A stacked branch whose base change landed as a rebased commit
  is NOT `conflicts-with-base` under `combine=rebase`, and still
  conflicts under `combine=merge`.
- A real conflict is still detected under `combine=rebase`: a
  head whose replay genuinely conflicts is held with the real
  conflicted paths.
- An exact merge whose base moved is ejected
  `not-fast-forward` (not held `conflicts-with-base`); an exact
  merge made on the current base still lands unchanged (existing
  exact tests keep passing).
- Existing probe, plan and intake tests keep passing, including
  probe-cache behaviour across a combine-mode or exactness change.

# Notes

Reported 2026-11-08 by a consuming project's merge queue (one
held change with a manual rebase-and-resubmit workaround);
consumers are referred to generically here, per repository
convention. The exact-item half is gap (2) from the #922
follow-up review: same probe, same fix. The other #922 gap —
label-driven sync cannot mark an item exact — is filed
separately.

## Resolution

Landed on main as 85478f473ec8ef830ae9389537d7cd794c5f5e18
(replay of the gated 5555c950 onto 91e4e621; tree identical).

`ProbeOptions` now carries the configured combine mode and the
exact-head set, threaded from the queue items at every ops call
site. Under rebase the base verdict replays through the existing
`combine_with` path (a commit whose patch is already on the base
drops out); under merge it remains plain merge-tree; exact heads
are never merge-probed, so a moved base reaches build's
`lands_exactly` ejection instead of a conflicts-with-base hold.
Exact cache rows are keyed separately so no verdict answers
across exactness. Covered by a probe unit (rebase-clean /
merge-conflict / real-conflict / exact cache separation) and a
`cli_plan` pair showing the stacked change held only under merge
combine.

Gate: full shared phase on the gated tip ran 234 passed / 4
failed, then the gate-exclusive cross differentials were stopped
under the merge-queue waiver (diff is `rs/merge-queue` only). The
four failures were adjudicated by one exclusive solo rerun, all
passing: two wrkslots shards (a bounded-scan budget timeout and
a host-state census mismatch), one vibe-talk cache race, and
`rust.merge-queue.test` killed at the old 1200 CPU-s budget with
no test verdict (shared profile: wall 426.490s, user 498.240 +
sys 702.519 ≈ 1200.8 CPU-s at the kill; the exclusive rerun
passed the whole crate clean, 557 passed / 0 failed, profile
elapsed 459.583s at user 215.176 + sys 661.717 = 876.9 CPU-s).
The budget itself was raised by #1051 in the meantime.
