---
title: 'cross-differential-load-aware: load-aware timing checks so the cross differentials leave the gate''s exclusive phase'
status: closed
priority: 1
issue_type: feature
created_at: 2026-11-08T15:20:00.000000000+00:00
updated_at: 2026-11-14T22:40:19+00:00
closed_at: 2026-11-14T22:40:19+00:00
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

## Resolution

[watermelon-full-preview] Closed 2026-11-14 as subsumed by #1249 cross-differentials-robust. The approach changed: instead of making the timing assertions load-aware (judging measured times against load or a same-run calibration, which still keeps a wall clock in the verdict), #1249 removed every wall-clock verdict from the five differentials outright — outputs and behaviour are compared, hang guards are generous and reported as hangs, and pure-speed checks leave gating. All five steps (`cross.agentctl.differential-0/1/2`, `cross.herdr-run.differential`, `cross.agentctl.herdr-agent-differential`) now run in the main phase with no `gate-exclusive` label; the last label-drop landed as 08fbe690 on 2026-11-14. The exclusive phase this issue targeted is therefore empty for these steps, and its acceptance criteria are moot in their original form; the qualification records (10 consecutive saturated-scope passes per differential, CPU-second runaway guards at 2x measured peak) live in #1249.
