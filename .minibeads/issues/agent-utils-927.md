---
title: 'merge-queue-empty-red: a red validation that recorded no test result is could-not-run, never an ejection'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-06T06:01:11.591507955+00:00
updated_at: 2026-11-06T06:01:11.591507955+00:00
---

# Description

In merge-queue's command validator, a command that writes nothing to $MQ_RESULTS and prints no dagrun-style step lines leaves an empty result set. validate.rs sets results.unattributed = true and for an all selection leaves the failed set empty; the exit status is then the only signal. run.rs culprit search treats that red like any other, so a single-change batch gets ejected as failed-tests with an empty failure list. Incident: a solo batch exited 1 with empty results and unattributed true after host tooling crashed before any test started; the change was ejected with no evidence against it.

# Design

1) validator.empty_red = could-not-run|red, default red. Under could-not-run an empty-result red reuses the existing could-not-run refusal path (no strike, requeued at same head, counted against validator.max_deferrals and escalated at the bound). 2) Never eject with an empty failed set: name it red with no recorded test result. 3) Bisection probes that are empty reds are Unknown/Inconclusive and cannot name a culprit.

# Acceptance Criteria

- Empty-result red under could-not-run: no strike, requeued at same head, counted toward escalation, escalates at bound. - Same under red: ejected with the named event. - Red with results still ejects normally under both settings. - Multi-change bisection unaffected by an empty red.
