---
title: 'merge-queue-plan-intake-parent: the plan-intake event claims every batch sits on the in-flight tip'
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-08T14:22:59.000000000+00:00
updated_at: 2026-11-08T16:47:43.000000000+00:00
---

# Description

The `plan-intake` event describes every replanned batch as
"on top of <tip> (carrying #N)", where <tip> is the last
in-flight candidate. That is not always true: a batch the fill
loop later builds as a disjoint root sits on the base itself and
its candidate does not contain the changes the tip carries.

A consuming project's run recorded exactly that: plan-intake
named a batch "on top of" an in-flight tip carrying one change,
while the batch's candidate was a disjoint root whose parent sat
on the old base and did not contain that change at all. The event
is the run's own audit of what was planned; an operator (or a
later diagnosis) reading it reconstructs a stack that never
existed.

# Design

- Name each batch's real parent and its kind in the plan-intake
  event, as the fill loop will decide it: `root on <base>` for a
  disjoint root, `stacked on #N's candidate` for a batch built on
  an in-flight or earlier-planned candidate. Never claim a batch
  carries (or sits on) a change its candidate does not contain.
- The prediction must use the same inputs the fill decision
  uses (in-flight flights and their rooted/stacked shape, the
  stack gate, root slots, stale-green stacking and `--after`
  dependencies), applied in pending order so later batches chain
  on earlier planned ones. Extract that parent/kind decision
  into one helper the fill loop and `plan_intake` both call (the
  pipeline passes its flights snapshot into `take_intake`), so
  the event cannot drift from the build it predicts; where the
  decision genuinely cannot be known at intake time, say no
  parent at all rather than a wrong one.
- Keep the event one line per intake (held count as today); the
  build-time events (`validate`, `disjoint-root`) remain the
  record of the candidate that was actually built, and the two
  must not disagree.


Implementation notes (from code reading): the wording lives in
`plan_intake` in `rs/merge-queue/src/run.rs` (one success event, two
"cannot plan on top of" error events at its call sites in
`take_intake` and the rulings path). The fill loop's rooted/stacked
decision (same file, where `Flight { rooted, stacked, .. }` is
chosen) is the decision to extract and share; the pipeline caller
already has the flights `plan_intake` needs a snapshot of.
Existing coverage to extend: `cli_confirm` (asserts plan-intake
details) and `cli_disjoint_root` (root scenarios).

# Acceptance Criteria

- A k-wide disjoint-roots plan-intake names the roots as roots
  on the base, not on top of the in-flight tip.
- A stacked batch is named stacked on the change whose
  candidate is its parent.
- Existing intake, disjoint-root and pipeline tests keep
  passing (event-text assertions updated where they pinned the
  old wording).

# Notes

Reported by a consuming project's merge queue from a live run's
event log; consumers are referred to generically here, per
repository convention. Queued after the recount refusals work;
may share its gate if ready together.

## Resolution

Landed on main as d3f107ade521b6ef663d4bc6295df5c197330ce3 (same stack as the recount refusals
change; one gate covered both, exclusive phase waived for the
merge-queue-only diff).

The plan-intake event names each planned batch's parent as the fill
loop will choose it — root on the base, stacked on a named
candidate, waiting for dependencies, or parent decided at launch —
computed by simulating the fill decision (dependency, stale-green
stacking/rooting, disjoint-root rule, green-root deferral, root-slot
and window limits) over a window snapshot threaded from both
pipeline intake sites. It never claims a batch sits on a candidate
it will not be built on. Covered by a k-wide disjoint-roots CLI test
(both intakes named roots on the base; no "on top of" text remains)
and the updated cli_confirm intake assertion.
