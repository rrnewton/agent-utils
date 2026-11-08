---
title: 'predict-first-submission: first submissions get the model base rate instead of a real prediction'
status: open
priority: 2
issue_type: bug
assignee: mu-predict
created_at: 2026-11-08T11:52:16.000000000+00:00
updated_at: 2026-11-08T11:52:16.000000000+00:00
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
