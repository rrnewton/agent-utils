---
title: 'predict-false-examples: training keeps decided examples for changes that had already landed'
status: open
priority: 2
issue_type: bug
assignee: mu-predict
created_at: 2026-11-08T15:52:20.000000000+00:00
updated_at: 2026-11-08T15:52:20.000000000+00:00
---

# Description

Decided predictor examples can be false. On a live queue, an intake bug re-queued a change two seconds after it landed; the next run ejected it, and that ejection is recorded as a decided example of the change - training now holds a false example against a head that had already landed, and history features derived from it (prior culprit, cleared share, author/component rates at scoring time) inherit the noise. `predict::examples` has label hygiene for red classes (infra / false-red / host) but nothing that recognises "this exact head was already landed when this validation ran".

# Design

- In `predict::examples`, drop a change-instance (per change, not per batch; kept siblings in the same batch still train) when the same `(pr, head)` was already landed before the validation started, or landed in an earlier validation of the same run. Landing evidence is earlier decisions' `landed` records (`LandedBy` prs plus that decision's change head), ignoring simulated shadow landings; empty heads never match (fail open, as today).
- Drop both polarities: a post-landing validation attributes nothing real about the change, green or red. The landing validation itself is always kept.
- History folding is unchanged (dropped instances still fold, as excluded reds do), so training and serving keep the same history construction and no train/serve skew opens.
- Report drops under their own model counter (`excluded_landed`, omitted while zero like `excluded_labels`) and render line, counted by the same shared helper `examples` uses, mutually exclusive with the red-class count.
- Pair-conflict rates keep today's inputs; revisiting them is out of scope here.

# Acceptance Criteria

- A red/green decision for an already-landed `(pr, head)` produces no training example, while the landing decision itself, pre-landing decisions, a later decision for a new head of the same PR, and kept siblings in a mixed batch all still produce examples (unit tests).
- Same-run ordering: a decision in a later validation of the run that landed the head is dropped; an earlier validation in that run is kept.
- `model train` output names the dropped count when non-zero, and the count equals the instances `examples` dropped (shared helper, no double count with infra/false-red/host exclusions).
- The submit-time-vector equality test still passes (serving history unchanged).

# Notes

Reported 2026-11-08 from a live queue (the false example came from an intake re-queue after landing). Companion to the disjoint-root stacking work filed the same day; separable because this is predictor label hygiene with no scheduling effect.
