---
title: 'dagrun-nextest-outer-wallclock: the nextest outer-backstop smoke test sizes its outer bound at 2 seconds'
status: closed
priority: 2
issue_type: bug
labels:
- dagrun
- flaky-test
created_at: 2026-11-23T14:05:00-05:00
updated_at: 2026-11-24T02:21:24-05:00
closed_at: 2026-11-24T02:21:24-05:00
---

# Description

`controlled_and_third_party_runners_leave_attributable_kill_evidence` in `rs/dagrun/tests/test_runner_integration_smoke.rs` runs its `nextest_outer` negative bracket with a 2-second outer DAG step bound against a nextest profile whose per-test timeout is 60 seconds. On a busy host, cargo-nextest startup (spawning the runner and the test binary) alone can exceed 2 seconds, so the outer bound fires before the planted hanging test exists as a process: there is no culprit to attribute, and the step fails with `tests.nextest_outer TIMEOUT >2s`. That is a wall-clock pass/fail criterion on host speed, which the hot-machine rule forbids. Seen in the #2162 gate on a loaded host (load well above 100); the same bracket passes on a quiet host.

# Design

The outer bound exists to prove the DAG backstop kills and attributes a test whose own runner timeout is mis-sized outside it; it must stay below the profile's 60-second per-test timeout, but it is a backstop, not a measurement. Size it generously (30 seconds: 15x the old bound, still 2x below the per-test timeout) so what fires it is the planted hang, not runner startup, and say so in the comment. The `nextest_inner` bracket's 5-second outer bound has the same shape in miniature; raising it costs nothing on the passing path (the run ends when the per-test bound fires), so size it the same way.

# Acceptance Criteria

- The smoke test passes on a busy host (load above 100) with the outer bound firing on the planted hang, still naming `tests::planted_wall_hang` with the exact-argv binding in the journal.
- No 2-second (or similarly host-speed) wall-clock bound remains a pass/fail criterion in this test.

# Notes

Filed from the #2162 gate triage at the coordinator's direction. Until this lands, `rust.dagrun.test` carries a narrow known-red entry naming this issue (not a glob), added with the #2162 landing so removing the `rust.*.test` blanket does not turn every gate red meanwhile.
