---
title: 'mq-probe-pass-stop-text: name an all-pass probe''s unqualified exit in the inconclusive text'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-12T16:56:28.991173975+00:00
updated_at: 2026-11-12T16:56:28.991173975+00:00
---

# Description

When a culprit-search probe is inconclusive, the run's reason has two texts: an empty red (no recorded result) names the missing result, and everything else says the probe observed only missing/skipped tests (no pass, no reproduced failure). A probe that passed every selected test but exited non-zero because its receipt did not qualify falls into the second text, so the report claims no pass for a probe whose own record shows every selected test passing. That sent the first diagnosis looking for a missing result that was never missing. The probe observation itself is correct (#1273 probe-no-counted-tests, closed: such a probe is inconclusive, never a red base and never a counted pass); only the words are wrong.

# Design

In rs/merge-queue/src/run.rs (CulpritSearch::record, the inconclusive branch) with a helper on Ran in validate.rs: give the all-pass unqualified probe its own text, for example: probe at prefix N of M passed every selected test but exited 4 (the validator's receipt did not qualify), which counts as no evidence. The exit in the text is the probe's actual exit status. The empty-red text and the missing/skipped-only text stay exactly as they are for the probes they describe. No observation, verdict or ejection behaviour changes; this is the wording only, so it lands after #1379 mq-unknown-probe-singleton has settled which all-pass probes eject and which stay inconclusive.

# Acceptance Criteria

An inconclusive all-pass probe with exit 4 reports the passed-every-selected-test text naming exit 4, and never the missing/skipped text. An empty-red probe and a missing/skipped-only probe keep their current texts word for word. cargo test -p merge-queue (all targets) passes, including a test pinning each of the three texts.

# Notes

Evidence: a 2026-11-12 drain report relayed to this lane: the run reported no pass for a base probe whose record shows every selected test passing with exit 4. Lands last of the five, after #1379 mq-unknown-probe-singleton, because both touch the same inconclusive branch; #1378 mq-trust-base-across-runs and #1377 mq-validator-purpose-env shrink how often the text is reached at all.
