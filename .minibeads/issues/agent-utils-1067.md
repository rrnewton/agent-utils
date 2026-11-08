---
title: 'cross-differential-load-aware: load-aware timing checks so the cross differentials leave the gate''s exclusive phase'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-08T15:20:00.000000000+00:00
updated_at: 2026-11-08T15:20:00.000000000+00:00
---

# Description

The gate's exclusive phase is the queue's biggest bottleneck: on 2026-11-08 one host saw 2-3 h waits at about 4 gates an hour, and every exclusive entry stalls the shared runs queued behind it. The five cross-language differentials run in the exclusive phase only because their wall-clock timing assertions are load-sensitive. The 2026-11-07 throughput study (harness repo ai_docs/2026-11-07-cross-differential-throughput.md) ruled out parallel-in-admission, more sharding and input-hash caching, and recommends making those timing assertions load-aware so the steps can run in the main phase.

# Design

- Make each of the five differentials' timing assertions load-aware: judge the measured time against host load, or relative to a calibration measured in the same run. No flake masking: a real regression (a genuinely slower path) must still fail.
- Move the five steps into the main (shared) phase.
- Keep a configuration switch that puts them back in the exclusive phase without a code change.
- Record what each assertion now compares, and why that still catches a regression, in the code and the commit body.

# Acceptance Criteria

- A test per assertion: an injected slowdown of the measured path still fails under load; normal runs under heavy load (calibrated, e.g. CPU burners) pass.
- A paired timing record under real gate load on both hosts: gate admission waits and gates per hour with the steps in the main phase versus the exclusive phase, plus the differentials' pass rate over at least 20 main-phase runs.
- The configuration switch is documented and tested in both positions.

# Notes

Filed by the devbig030 coordinator at the lead's request (2026-11-08); implemented by a fresh lane from mu-validate's handoff.
