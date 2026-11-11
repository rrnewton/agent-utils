---
title: 'probe-candidate-only: skip the culprit probe when every failing test is absent from the base'
status: closed
priority: 0
issue_type: bug
created_at: 2026-11-11T10:05:00.000000000+00:00
updated_at: 2026-11-11T10:05:00.000000000+00:00
---

# Description

Observed: singleton change #A (--solo) went red at 09:48:26Z after 954 s on node test.cli. Its one failing test is NEW in #A, so it does not exist at the base. The queue logged "confirm-probe #A is red; its culprit search runs as 1 window flight(s)" and published "validate v2 probe (nothing) on <base>: 1 test (failed-tests)". That became a donor job plus a compare-only duplicate, and the donor admitted and ran it. The probe cannot decide anything for a test absent at the base. Everything queued behind waited 13.7 min, until the operator withdrew #A at 10:02:08Z ("result v2 cancelled in 821.7s").

# Design

In `rs/merge-queue`:

- Before publishing a culprit probe, compare the failing test ids with the base census listing ("CENSUS: suite=... base=N candidate=M").
- Attribute candidate-only failing tests to the change directly; don't probe them.
- If every failing test is candidate-only, eject at once as failed-tests ("its own new test failed").
- Probe only the failing tests that also exist at the base.
- If the base census listing is missing or unreadable, keep today's behaviour and probe all.

# Acceptance Criteria

- All candidate-only failures: no probe is published; the singleton ejects at once as failed-tests.
- Mixed failures: only the base-present ids are probed.
- Missing or unreadable base census: today's behaviour (probe all).
- `cargo test -p merge-queue` (all targets) passes, including the new cases.

# Notes

Full report: the 2026-11-11 probe candidate-only report relayed to this lane (queue on ce0db627). Worked example from the report: the operator withdrawal cancelled the probe flight and retired both donor jobs.
