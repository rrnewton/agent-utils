---
title: 'mq-singleton-eject: do not eject a singleton for a red that is the base'
status: open
priority: 0
issue_type: bug
created_at: 2026-11-10T03:34:52.000000000+00:00
updated_at: 2026-11-10T03:34:52.000000000+00:00
---

# Description

P0 report (queue runs 20261109T220400Z and 20261110T020453Z). The merge queue ejects a singleton batch (one change, culprit index 0) with "fail with #PR on top of the base and pass without it" when the base itself is red and was never measured. With `trust_base: true`, `CulpritSearch::new` inserts a synthetic pass for prefix 0, so "pass without it" is assumed, not measured. When the base moved by direct push rather than a validated queue landing (freeze pushes ending at 91548b41; tonight a direct-push base 19b45f66 that no candidate validated; one more change ejected the same way), a base red is blamed on the singleton. Two disjoint singletons on the same base failing the same test were both ejected for a line on the base that neither touched.

# Design

In `rs/merge-queue/src/run.rs` (`CulpritSearch`, its verdict, and the red-batch finish path):

- Before ejecting a singleton (index 0) for failed tests T, require evidence T passes on the base: a recorded green for T at the base tree, or a validated queue landing of that base. Otherwise probe T on the base alone and eject only if the probe passes. A synthetic `trust_base` observation counts as no evidence when the base is not a queue landing (a direct push to the base branch); `trust_base` keeps working for bases the queue itself landed after validating.
- Two or more disjoint batches failing the same test on the same base mark the base suspect: hold those batches, start nothing new on it, and probe it. Do not eject.
- Honest text: when the base observation is synthetic, the ejection text says "pass without it (assumed: trust_base)"; when the base is not a queue landing, say so in the event, the verdict detail, and `runs/<run>/ejections/<pr>.md`.

# Acceptance Criteria

- Base B fails test T alone; two disjoint singletons on B both fail T: no ejection, a base probe or base-suspect hold is recorded, and neither change is blamed.
- With `trust_base` on, a direct-push base failing T and a singleton failing T: probe or hold, with no "pass without it" claim.
- A queue-landed base is still trusted as today (no extra base probe before a singleton ejection).
- `cargo test -p merge-queue` (all targets) passes, including the new cases above.

# Notes

Spec and evidence: the 2026-11-09 handoff report for this queue (code read at dab2a70c; `Culprit::Found` near run.rs:9320, synthetic base at `CulpritSearch::new` near run.rs:994). An earlier lane left unrelated merge-queue work on other branches; do not build on it.
