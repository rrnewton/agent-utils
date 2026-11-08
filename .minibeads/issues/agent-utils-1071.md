---
title: 'disjoint-root-stacking: a root that jumps over a batch likely to land first should stack on it instead'
status: open
priority: 2
issue_type: feature
assignee: mu-predict
created_at: 2026-11-08T15:52:20.000000000+00:00
updated_at: 2026-11-08T15:52:20.000000000+00:00
---

# Description

Disjoint-root promotion never rescues a green whose base moved. `promote_decidable_roots` requires `candidate.parent == base` plus `rooted_decidable`, so a root becomes decidable only when everything it jumped over left the window without landing. On a live queue on 2026-11-08 one solo change validated green twice as a root (about 1180 s and about 1109 s of donor time) and both greens were discarded as `stale-root-green` because another batch landed meanwhile - roughly 38 minutes of validation for one change, then it still had to validate a third time. The root build itself was the wasted work: everything it jumped over was likely to land first, so building it on the base guaranteed the base would move under it.

# Design

- Option (a) only: when a batch would be built as a disjoint root on the base, and a batch it jumps over is likely to land first, stack it on that batch's candidate (the stack top when one exists, else that candidate) as an ordinary stacked flight instead of rooting it. Likely means any of: the jumped-over batch is prioritized, it already holds a green result awaiting decision, or its predicted batch pass probability (`plan_ev::batch_p_pass` over the predictor/rate estimates) is at least the configured percent.
- Behind config with today's behaviour as the default: `root_stack_min_pass_percent` (0 = off). The predicted-pass estimates reuse the expected-value context construction (trained model when present, historical rates otherwise) without enabling the EV planner's ordering or aborts.
- The stacked result is encoded as a plain stacked flight (`rooted=false`), never a root on a non-base parent, so stale cancellation, discard and promotion predicates need no new cases. Only when stacking is allowed by the stack pass-rate gate, there is no dependency tip, and it is not the prioritized stale-green root case.
- A simulate case in `plan_ev` models base-move root discards (a root whose jumped-over batch lands first wastes its validation and rebuilds) and shows the stacking policy rescuing it, reproducing the two-discard waste shape.
- Option (b) - landing a stale green without a rerun when everything landed since is file-disjoint - is an owner policy question (the pending level-0 delta validation decision) and is explicitly not built here.

# Acceptance Criteria

- With the config at its default, root construction is byte-for-byte today's behaviour (existing disjoint-root tests unchanged and passing).
- With the config on, a root whose jumped-over batch is prioritized, already green, or predicted at or above the percent threshold validates as an ordinary stacked batch on that candidate, lands after it without a second validation, and the run report names the choice and its reason.
- The simulate case counts discarded roots under the base policy (at least two discards in the two-landings scenario) and none under the stacking policy, with strictly fewer validate hours wasted.
- Unit tests pin the likely-landing decision (prioritized, already-green, threshold boundary, config off) independent of the run loop.

# Notes

Reported 2026-11-08 from a live queue after the success predictor was enabled there. Related landed work: the green-root base-choice deferral (up to 60 s) already covers a jumped-over root that is green and about to land; this covers likely-to-land batches generally. Companion label-hygiene work is filed separately.
