---
title: 'merge-queue-kept-greens-prefix: kept greens re-validate after the first one lands and moves the base'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-07T14:19:08.000000000+00:00
updated_at: 2026-11-08T10:36:49.000000000+00:00
---

# Description

Greens kept from a stopped run carry into the next run only while the
base is unchanged. When the first kept green lands, the base moves, so
every remaining kept green is re-planned as fresh work and re-validated
from scratch. In the reported run (2026-11-07), a new run landed the
first kept green on the unmoved base, then re-planned four more kept
greens as fresh work: about 20 minutes of re-validation, with a fresh
flake risk on work that had already been validated green. This caps
the landing cadence of a consuming project's queue after every stop.

# Design

- After a kept green lands, check the remaining kept greens in
  sequence. A kept green still lands without re-validation when what
  has landed since its base is exactly its validated prefix: the merge
  result equals the validated tree, so the tree that was validated is
  the tree that lands.
- Anything else is re-validated as today. In particular there is no
  disjoint-root landing on a moved base: a rooted batch is never landed
  on a base other than its parent (the stale-root-green invariant),
  because that merged tree was never validated.
- Withdrawn kept greens are dropped, not landed (the withdrawn-batch
  handling already on main); a kept green whose prefix includes a
  withdrawn change is re-validated or set aside by the same rules as
  any other non-matching prefix.

# Acceptance Criteria

- Kept greens v7..v12 with a stop: v7 lands, and v8 (whose validated
  prefix is exactly v7) lands without re-validation.
- A kept green whose landed-since-base history is not exactly its
  validated prefix — disjoint or overlapping, for example a disjoint
  v10 behind a red v9 — is re-validated, as today.
- A withdrawn kept green never lands from its kept green.
- Existing merge-queue behaviour is otherwise unchanged: greens kept
  while the base is unmoved, stale roots, refusals and culprit
  searches behave as before.

# Notes

Reported 2026-11-07 by a consuming project's merge queue; consumers
are referred to generically here, per repository convention. Rule (1)
only (exact validated prefix); the disjoint-root variant was
considered and dropped because it would land a tree that was never
validated.

## Resolution

Landed on main as bf0f68523c1a3d6181f55bc1237f8d0dc0326e26
(6767d954, 607bc18c, bf0f6852; replays of the gated branch commits
fb700d73, e979bbd4, 85dc6349 onto b9bd47d1).

`land_kept` now checks kept entries in order: an entry lands without
re-validation when its landed changes are exactly an in-order
validated prefix and the remainder rebuilt on the current base has
the validated tree and selection. Anything else re-validates as
before; withdrawn kept greens still never land from the kept record.
Covered by `rs/merge-queue/tests/cli_push_retry.rs`.

Gate: the shared phase of the full `--all` gate passed clean on the
gated tip (dagrun 238 passed / 0 failed; merge-queue step 549
passed / 0 failed). The 5 gate-exclusive cross differentials were
waived by the coordinator because the diff touches only
`rs/merge-queue`, which component selection says cannot affect those
tools; the queued exclusive ticket was stopped after the main
phase. Main had moved since the gated base and touched
`rs/merge-queue/src/run.rs` (donor/identity field initializers,
disjoint from the kept-greens hunks), so the combined tree was
verified separately (clean replay, all three patches identical by
range-diff, fmt and clippy clean, full merge-queue crate 555
passed / 0 failed) and landed with `--replay-base --allow-overlap`;
the landed tree is byte-identical to that verified combined tree.

### The prioritized stale-green test race (found while landing)

`a_prioritized_stale_green_roots_instead_of_stacking_under_a_plain_head`
timed out once in a 5x solo pre-check (242s, "timed out waiting for
the prioritized validation") and reproduced on loop iteration 1 with
the identical signature. Live inspection showed the spawned
`merge-queue run` already zombie while the test polled: the first
`wait_for` read the *seed* run's events (the only run directory at
that point, already holding three validations), so it returned
instantly and the prioritized `submit` raced the starting run for
the fail-fast writer lock. When submit won, the run exited before
creating its run directory; the test then polled seed events until
the 240s timeout, and fixture drop deleted the evidence. Load only
widened the run's startup latency; the defect was the wait's false
premise.

Fix (85dc6349, test-only): waits read only the spawned run's events
(its run directory exists only once it holds the writer lock, which
is what makes the submit take the intake path) and fail fast if the
child exits; change 1 is gated on `release-a` so it lands only
after batch #2 launches on the original base; depth is 4 so intake
always has a free window slot (a run takes intake only while
occupied < window — two intermediate designs deadlocked on exactly
this rule and on flight-order decidability); change 4 arrives by a
second intake after change 1 lands, making it the plain stack head
on the new base behind change 2 in flight order.

Mutation check that the reworked test still tests the product:
reverting only the product half of 91791506
(`rs/merge-queue/src/run.rs`, 9 insertions / 124 deletions) in a
scratch worktree makes the reworked test FAIL (exit 101, 35.20s):
events show `stale-root-green #2` then `stale-green-stacked #2`
(stacked on 24bb85a7d3) and #2 landing stacked, tripping the
"prioritized stale green rooted" assertion. On the gated tip the
same test passes 5/5 consecutive full-log solo runs (39/37/35/36/36s).

A residual ordering race in the same test surfaced in the
bookkeeping validation after landing: it wrote `release-d` on
observing change 2's stale requeue, but the rebuild had not
launched yet, so change 4 (a promotable root) could validate and
land first, dissolving the stale decision into an ordinary root
rebuild on the new base. 198c4633 waits for the rebuild launch
itself (the second batch #2, when rooted-vs-stacked is decided
while change 4 is still gated) before writing `release-d`.
Verified with 10/10 consecutive solo passes (35–37s), the same
mutation check repeated (FAIL exit 101 in 36.77s at the rooted
assertion, `stale-green-stacked #2`), and an exclusive gate
rerun of `rust.merge-queue.test` on the exact push tree (555
passed / 0 failed).

Related: #1021 merge-queue-load-sensitive-tests tracks the remaining
wall-clock-oracle tests; this race was of the same family (progress
assumed from timing rather than observed from the run's own events).
