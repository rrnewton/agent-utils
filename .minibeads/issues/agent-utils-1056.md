---
title: 'predict-first-submission: first submissions get the model base rate instead of a real prediction'
status: closed
priority: 2
issue_type: bug
assignee: mu-predict
created_at: 2026-11-08T11:52:16.000000000+00:00
updated_at: 2026-11-08T15:37:10.000000000+00:00
closed_at: 2026-11-08T15:37:10.000000000+00:00
---

# Description

With `Config::predict` on, `decisions::from_report` scores a change only from `predict::latest_vectors`, which builds vectors solely from already-decided training examples. A change on its first submission has no such vector, so it gets `model.base_rate` verbatim. Most changes in a drain are first submissions, so `predicted_pass` is non-null but identical for nearly every change. On two live queues on 2026-11-08 every scored change equalled its model's base rate, and `merge-queue stats` calibration was worse than predicting the base rate outright (for example Brier 0.333 against a 0.184 base-rate Brier, with a large bin predicted near 0.36 but observed near 0.82). The model can discriminate in training; the serving path never asks it about the change itself.

# Design

- Extract one shared vector construction in `predict` used by both training (`examples`) and serving: change-local features from the change's own `Features` (lines, files, components, conflict degree, peripheral, prioritized, CI-only, claim flags, width), plus history features from a snapshot folded only from decisions strictly before the scored validation (prior validated count, prior cleared share, prior culprit count, shrunk author and first-component rates, global rate).
- Neutral rule for no history, identical in training and serving: prior validated and culprit counts are 0, prior cleared share is the snapshot global rate, author/component rates shrink to that global rate, and resubmissions count only `head` signals strictly before the scored validation (0 on a first submission).
- `decisions::from_report` builds the vector from the `Features` it already computed for the change plus that snapshot and its resubmission count; it no longer falls back to the base rate for a change with no decided history. `Sources` carries the snapshot and resubmission counts instead of prebuilt per-change vectors.
- No new features and no scheduling change: the 16 `predict::FEATURES` stay as they are (later features can slot into the shared constructor), and the expected-value planner keeps its existing inputs.

# Acceptance Criteria

- A first-submission change whose change-local features differ from the training mean gets a `predicted_pass` different from `model.base_rate` (unit test).
- For an already-decided change, the submit-time vector rebuilt from its recorded `Features` and the pre-decision history snapshot equals the vector `examples` used for training (unit test); any intended difference is documented in code.
- Before/after evidence on a fixture or a copy of real decision history (never live queue state): the `predicted_pass` distribution spreads off the single base-rate value, and `model eval` calibration on the same data is reported before and after.
- Training, `model eval`, and decision scoring all call the same vector constructor; `cargo test` for merge-queue passes apart from any documented known flake.

# Notes

Reported 2026-11-08 from live queues that had just enabled the predictor built on 2026-11-05. Out of scope: new features such as pin or risk-path signals, and any change to the expected-value planner or scheduling.

## Resolution

[watermelon-full-preview] Landed on main as 23dcec839c296cdbc00bb4ab4bb7460e38ba54d6 (gated commit 0338a76d on the 7577cf67 budget fix, replayed onto a99c66ca). Training and serving share `predict::feature_vector`; `predict::ServeContext` (pre-run history snapshot plus head times) replaces `Sources::model_vectors`, the base-rate fallback arm is gone, and resubmissions count `head` signals strictly before the scored validation in both constructions. The one intended train/serve difference - serving fixes its snapshot before the run while training folds each decision before later ones - is documented on `ServeContext`. The expected-value planner is code-untouched (`latest_vectors` remains for it alone; its values can shift from the causal resubmissions fix). Operators should retrain (`merge-queue model train`) after taking the new binary, since an old `model.json` was fit with the previous resubmission counts.

Evidence: the new `from_report` test failed on the old code with exactly the base rate and passes after; a decided change's submit-time vector equals its training vector (including causal resubmissions); first submissions spread off the base rate with a better Brier than the old fallback. On a 60-first-submission fixture with old and new binaries, `model.json` and `model eval` were identical (Brier 0.001 - eval re-derives training vectors, which is why the bug hid), while served `predicted_pass` went from 60 at base rate 0.5 (Brier 0.250) to 0.027-0.990 (Brier 0.001). Full gate: main phase 234 passed with 4 unrelated single-test failures (agentctl, vibe-talk, wrkslots lifecycle), all passing on an exclusive rerun; `rust.merge-queue.test` passed (598 s wall, 1432 CPU-s under the 2400 budget); the exclusive cross-differential phase was waived for this merge-queue-only diff.
