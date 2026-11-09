---
title: 'merge-queue-root-stack-mismatch: root stacking judges one batch but builds on another'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-09T13:23:24.000000000+00:00
updated_at: 2026-11-09T17:44:53.000000000+00:00
---

# Description

Found 2026-11-09 by a consuming project's merge-queue lane
(relayed through its coordinator; consumer names redacted per
repository convention), reading run.rs at dab2a70c against a
live run (20261109T094648Z) and confirmed in the code there.

Background: a *root* is a batch built on the queue base itself,
disjoint from the batches in flight. *Root stacking* (#1071) builds
a would-be root on the current stack top instead when some batch
in flight is "likely to land first", on the idea that the base is
about to move, so a root built on it would only go stale.
`plan_ev::likely_landing_reason` counts a batch likely when it
is already green, is prioritized, or has predicted pass at or
above `root_stack_min_pass_percent` (70 on that queue).

What happened (event 2026-11-09T11:18:35Z): a new would-be
root (prioritized) was recorded as stacking on the stack top
("stacks on <stack-top> (likely to land first: <another batch>
- prioritized, predicted pass 0.50) instead of rooting on the
base it is likely to move"). The window at that moment:

- the stack top: a stacked flight on the base, not prioritized,
  no verdict yet, its own predicted pass below 0.70, so the loop
  skipped it;
- a second flight: rooted, prioritized, with `jumped_over`
  naming the stack top.

The new batch was built on the stack top's candidate (its donor
job's parent is the stack top's candidate, not the likely
flight's).

Why it is wrong: in run.rs the loop walks the flights and stops
at the first one whose `likely_landing_reason` returns a reason
— here the rooted prioritized flight — and then takes the
parent from `stack_top()` (the last stacked flight), here the
unjudged stack top. The flight that passed the test and the
flight that becomes the parent are different batches. The
likely flight's priority says nothing about whether the stack
top lands: a rooted flight is decided only after every batch it
jumped over is decided, and is cancelled if any of them lands,
so the new build is useful only if the stack top lands, and the
likely test was never applied to it. If the stack top is red,
the new build is thrown away. The event line's `likely_prs` and
`parent_prs` naming different batches is the symptom. (An
earlier consistent case the same night: the batch that passed
the test was also the parent, after the stack head was promoted
when a batch in between was withdrawn.)

Impact: prioritized queues with a stacked flight in the window
compose speculative builds on batches nothing vouched for:
 wasted validations and discarded greens whenever the stack top
fails, for exactly the changes marked most urgent.

# Design

Apply the likely test to what the new build depends on:

- When a stack top exists, only the stack top is tested
  (`likely_landing_reason` over its own predicted pass,
  prioritized flag and green state, with the existing
  known-not-green skip). If it is likely, the new root stacks on
  it and the event names the same batch as both `likely_prs`
  and `parent_prs`. If it is not likely, the would-be root does
  not stack at all — it roots on the base as before, however
  likely any other flight is. A likely root outside the stack
  never justifies stacking on the stack top.
- When no stack top exists, the current behaviour stands: any
  likely flight may justify stacking, and stacking on it stays
  under the existing guard (a likely root's tip is used only
  when that root's parent is the current base; a cut-out or
  stale tip never is).
- Testing the stacked flights below the top as well is equally
  correct and deliberately not done: the new candidate contains
  the stack top's tree either way, so its fate is the binding
  one, and stacking only on a likely top keeps the rule and the
  event line to one batch.
- The fix is local to the root-stacking decision in the fill
  loop; parent choice for every other construction (dependency
  composition, stale-green stacking, the root-near-landing
  deferral) is unchanged. #1111's base recording is untouched
  and both shapes keep it right by construction: a disjoint
  root records parent == base at once, a stacked batch inherits
  the base of the flight it stacks on; the tests below pin both
  readings (`MQ_PARENT`/`MQ_BASE` per shape, as the guide
  defines them).

# Acceptance Criteria

- A window with a non-likely stacked flight (gated, plain,
  predicted below the threshold) and a prioritized rooted
  flight: a new would-be root is planned as a disjoint root on
  the base (`disjoint-root`, candidate parent equal to the
  current base), not `disjoint-root-stacked` on the stack top.
- A window whose stack top itself is likely (prioritized):
  the new batch still stacks on that top, validated after it,
  with the event naming the top as both the likely batch and
  the parent.
- The #1071 cases keep their behaviour: stacking on a
  prioritized stack head, and no stacking anywhere at the
  default threshold of 0.
- Existing disjoint-root suites keep passing; the whole
  merge-queue crate stays clean.

# Notes

P1 for the reporter's queue throughput, after #1119 in this
lane's chain. Reporter details (window composition, event
text, donor job parent) are generalized here; run
20261109T094648Z is from that queue, read-only. Test home:
cli_disjoint_root, beside the #1071 cases.

## Resolution

Landed on main as af3062c2e4a7bbd3be7a54ef57c1baddf3aae21a (gated commit 78c37719, files byte-identical after replay).

The root-stacking loop now judges only what the new build
depends on: when a stack top exists, `likely_landing_reason`
runs on the stack top alone (its own green state, priority and
predicted pass, known-not-green still skipped), so with a stack
top the event's `likely_prs` and `parent_prs` are the same
batch by construction; when no stack top exists, the previous
any-flight rule stands under the existing current-base guard
(a likely root's tip is used only while its parent is the
current base). A likely root outside the stack can no longer
make a new root stack on an unjudged top, and the stacked
flights below the top are deliberately not judged — the
candidate contains the top's tree either way. Parent choice
for dependency composition, stale-green stacking and the
root-near-landing deferral is unchanged, and both shapes keep
#1111's base recording (disjoint root: parent == base at once;
stacked: the stack's bottom inherited).

Tests in cli_disjoint_root, both shapes with their
`MQ_BASE`/`MQ_PARENT` readings: a non-likely stacked flight
(dep-composed, gated, plain) beside a prioritized root no
longer makes a new would-be root stack — it roots on the base
(failing on the old code, which built `a_b_d` on the top's
candidate justified by the root), and a prioritized stack top
still stacks with the event naming the top as both batches.
The #1071 cases pass unchanged. Gate: full gate on 78c37719,
shared phase 239/239; exclusive cross differentials stopped
under the merge-queue-only waiver (ledger `failed_steps`
empty, verdict fail only from that stop); independent review
of the commit was clean before gating.
