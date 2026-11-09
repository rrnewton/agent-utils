---
title: 'merge-queue-exact-clear: submit --exact is sticky with no way off, and an inherited exact is invisible'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-08T23:00:04.000000000+00:00
updated_at: 2026-11-09T01:14:42.000000000+00:00
---

# Description

`submit --exact` sets an item's `exact` for good. Resubmission keeps
it (`exact: submission.exact || old.exact` in `submit_resolved`),
there is no flag to clear it, and the submission record names exact
only when that submission itself passed `--exact`, so an inherited
exact is invisible to the operator.

Reported from a consuming project's queue: a change submitted
`--exact` was resubmitted without it so the queue would replay it
onto a moved base; the queue still treated it as exact and ejected
it seconds later as not-fast-forward, costing a rebase cycle. The
same stickiness applies to the exact label #1054 adds to `sync`:
sync can set exact from the label but removing the label can never
clear it, unlike the priority label, which sync follows both ways.

# Design

- Add `submit --no-exact`: an explicit clear of a recorded exact,
  logged as `exact cleared`. A plain resubmission keeps inheriting
  exact rather than the flags becoming authoritative: resubmission
  is also the head-move and intake path, and silently changing
  landing semantics because a flag was omitted is the worse
  surprise; with the receipt below naming an inherited exact, the
  omission is visible and the clear is one flag. `--exact` with
  `--no-exact` is refused.
- A live peer requirement holds exact against a clear. A peer
  change is marked exact when another queue submits
  `--requires PEER#PR` against its current head, so the item
  records that pinned head (`exact_peer_head`); while it names the
  item's current head, `--no-exact` and sync label removal keep
  exact and say so instead of silently overriding the dependent's
  pin. (The requiring repository cannot be named from the required
  side: peer configuration is one-way.) A resubmission at a new
  head is not held: the requirement pins the old commit.
- Every submission record (and the `submit` output) states the
  effective exact value and its source while it is true, and
  nothing when false: `exact: yes (submitted --exact)`,
  `exact: yes (inherited from an earlier submission; --no-exact
  clears it)`, `exact: yes (sync exact label)`, or `exact: yes
  (another repository requires its exact head; --no-exact cannot
  clear it)`.
- `sync` follows the exact label (and the ready-file `exact`
  word) both ways, as it follows the priority label: a managed
  entry with the label absent clears exact, subject to the peer
  hold above, and a peer-held item counts as settled so reconcile
  does not resubmit it every cycle.
- Update `submit --help` (including the `--exact` text that today
  says it "stays set on resubmission" with no way off) and the
  user guide's exact and sync-label sections.

# Acceptance Criteria

- Resubmitting plain after `--exact` keeps exact and the receipt
  says it is inherited; resubmitting with `--no-exact` clears it
  (item not exact, `exact cleared` in its history) and the change
  can then be replayed onto a moved base.
- `--no-exact` against an item whose current head a peer
  requirement pins keeps exact and says a peer requirement holds
  it; after the head moves the hold no longer applies.
- Removing the sync exact label clears a label-set exact on the
  next reconcile; a peer-held item keeps exact and reconcile
  leaves it settled.
- The receipt texts above are asserted for each source, and no
  exact text appears when exact is false.
- Existing submit, sync, peer and intake tests keep passing
  (whole merge-queue crate clean).

# Notes

Folded into #1054 merge-queue-sync-exact by the coordinator: the
two change the same exact-persistence code, and #1054's label is
only observable once clearing works. Reported by a consuming
project's queue; consumers are referred to generically here, per
repository convention. Gate as a product change under the
merge-queue rules (full shared gate phase; exclusive phase waived
for a merge-queue-only diff), landing with #1054 as a second
commit in the same push.

## Resolution

Landed on main as 3592b245d1ea61eca5343e38cabf06946058e418 (gated commit b84fc925, stacked on #1054's 3b79d2ca in one gated push).

`submit --no-exact` clears a recorded exact (history: `exact
cleared`); a plain resubmission keeps inheriting it, since
resubmission is also the head-move path and an omitted flag must
not silently change landing semantics. Every submission's output
and record now name the effective exact and its source while it
is set (`submitted --exact`, `inherited from an earlier
submission; --no-exact clears it`, `sync label`, `sync ready
file`, peer-held), and say nothing when it is not. A peer
requirement records the head it pins (`exact_peer_head`, set by
`--requires` resolution and the busy-lock mark-exact intake;
pre-existing history marks count too) and holds exact against a
clear while that is the item's head, saying so in the receipt; a
new head is not pinned. `sync` follows its exact marking both
ways under the same exception, and a peer-held item counts as
settled in reconcile. Covered by cli_peers tests (inherit, clear
then replay onto a moved base, peer hold until the head moves)
and the cli_sync label-removal test. Gate: full shared phase on the gated stack (dagrun 238 passed / 1 failed; rust.merge-queue.test clean at 585 passed / 0 failed across 51 binaries, wall 534.312s at user 519.555 + sys 776.082 CPU-s). The one failure was untouched timeline.browser.functional, refused in 1s because another lane's server held its health port, and it passed solo in an exclusive rerun (38 passed in 49s). The gate-exclusive cross differentials were stopped under the coordinator waiver for this merge-queue-only diff. Main had moved one overlapping merge-queue change set (plan-intake naming, run.rs/USER_GUIDE/cli/state context) since the gated base; the replay is patch-identical, and the combined tree passed the whole merge-queue crate (591 / 0) with fmt, clippy -D warnings and the client-names check clean; its merge-queue paths are byte-identical to the landed tree.
