---
title: 'mq-unknown-probe-singleton: eject a deterministic singleton on an all-pass base probe and hold only its batch when inconclusive'
status: open
priority: 0
issue_type: bug
created_at: 2026-11-12T16:53:42.138409162+00:00
updated_at: 2026-11-12T16:53:42.138409162+00:00
---

# Description

A one-change batch whose red is deterministic (every failure in a step the validator names deterministic, so a rerun of the same tree cannot change it) is never ejected when the base probe is inconclusive. The probe of the base alone can observe every selected test passing yet still count as no evidence, because the validator exited non-zero: its receipt did not qualify (a targeted run can never qualify as a full green, so it exits 4 even when every selected node passed). The culprit search then names no culprit. Separately, one inconclusive culprit search stops the whole run: every other independent flight in the window is cancelled or detached, discarding minutes of validation that had nothing to do with the inconclusive batch, and the changes behind it wait for a fresh run. Observed in a 2026-11-12 queue drain: a singleton failed alone on a deterministic static check whose offending line was only in its own diff; the base probe passed every selected test but exited 4, the search stopped the run, and two unrelated in-flight validations (about ten minutes each) were cancelled with it. The singleton was finally withdrawn by hand. Prior work: #1187 mq-singleton-eject (closed) requires measured base evidence before blaming a singleton and probes a non-landing base first; #1273 probe-no-counted-tests (closed) made an all-pass non-zero-exit probe inconclusive rather than a red base; #1283 probe-candidate-only (closed) ejects at once when every failing test is absent from the base. None of them ejects the deterministic singleton whose base probe passed every selected test, and none keeps the rest of the run alive.

# Design

In rs/merge-queue/src/run.rs (CulpritSearch::record, Runner::finish_red) and rs/merge-queue/src/validate.rs (Ran::probe_observation helpers): (a) When the prefix-0 probe of a one-change batch passed every selected test but exited non-zero (the all-pass unqualified case, distinct from an empty red and from missing/skipped-only results), and the batch's red is deterministic (confirmed on the deterministic marker, not merely seen once), count the probe as base-pass evidence for that singleton: record the pass at prefix 0 so the search finds the culprit at index 0, and eject as failed-tests. The ejection text and event say the base probe passed every selected test but its receipt did not qualify (naming the exit), so the evidence is never overstated. This applies only to a deterministic singleton: a non-deterministic red, a multi-change batch, an empty-red probe, and a missing/skipped-only probe all stay inconclusive, and the base-suspect hold (two disjoint batches failing the same test on one base) still wins over ejection. (b) An inconclusive search no longer stops the run: finish_red returns the deferred outcome (the batch's changes stay queued with the inconclusive reason recorded) instead of the stop outcome, reusing the held-front handling so disjoint flights behind it are decided and land in the same run and stacked batches rebuild on the base. Nothing is ejected on an inconclusive search, exactly as today.

# Acceptance Criteria

A deterministic singleton whose base probe passed every selected test but exited non-zero is ejected as failed-tests, with text naming the unqualified exit; no run stop is recorded for it. The same singleton with a non-deterministic red, or with an empty-red or missing/skipped-only base probe, stays queued (held with the inconclusive reason) and ejects nothing. A multi-change batch with an inconclusive probe ejects nothing and stops nothing. With an inconclusive batch in the window and an independent green flight behind it, the green flight lands in the same run. cargo test -p merge-queue (all targets) passes, including the new cases.

# Notes

Evidence: a 2026-11-12 drain report relayed to this lane (queue operator's run records; code read at the deployed merge-queue revision). Three base probes in that evidence all passed every selected test and exited 4; each stopped its run. Related: #1196 front-starve (open) is the same shape of loss (a blocked front discarding independent green work) on the claim/decision path; this issue is the culprit-search path. The #1362 agent-utils-merge-queue-dogfood plan's notes make validator signals the blocker for wider queue use; inconclusive handling that keeps independent work moving is part of that signal quality. Fix first with #1381 mq-submit-approval-gate (safety/progress pair), before #1377 mq-validator-purpose-env.
