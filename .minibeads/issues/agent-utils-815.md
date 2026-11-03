---
title: 'herdr-run-e2e-readiness-race: the herdr-run differential''s readiness cases diverge under normal load'
status: open
priority: 2
issue_type: bug
assignee: mu-validate@devbig030
labels:
- herdr-run
- validation
created_at: 2026-11-03T11:40:29.473065688+00:00
updated_at: 2026-11-03T11:40:29.473065688+00:00
---

# Description

[opus 5.5] cross.herdr-run.differential fails intermittently in its end-to-end readiness cases under normal host load, with no other gate on the host: e2e/readiness-half-typed-line and e2e/reply-id-not-a-string (and, earlier, e2e/readiness-busy). In the handed-to-herdr call sequence the Python edition records a pane read (ignored=[3,32,33]) where the Rust edition records a pane run (ignored=[]). Seen in run 3 of the serial solo-phase stability runs (2 of 1868 cases) and once under the old one-step-at-a-time solo phase, so it predates the admission change: a readiness-probe race inside the step's e2e fixture or in one edition's readiness logic.
Until it is fixed, a lone failure of that step in these cases may be rerun once solo (devbig014 coordinator's rule, 2026-11-03).

# Design

Root-cause the race in cross/herdr_differential.py and the two editions' readiness paths: find which side decides to read versus run the pane and what timing it depends on (wall-clock probe windows, the fake Herdr's response timing). Fix the fixture so both editions see the same deterministic readiness sequence, or fix the edition whose decision depends on timing; follow the timeout rule (CPU budgets, wall clock about 8x) rather than lengthening sleeps.

# Acceptance Criteria

- The root cause is written in the issue before the fix.
- The affected cases pass 50 consecutive runs of the step under normal host load.
- No case is skipped or given a longer sleep as the fix.
