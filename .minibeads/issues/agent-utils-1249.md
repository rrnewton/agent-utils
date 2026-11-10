---
title: 'cross-differentials-robust: bring back the cross differentials without wall-clock budgets'
status: open
priority: 0
issue_type: feature
created_at: 2026-11-10T21:23:00+00:00
updated_at: 2026-11-10T21:23:00+00:00
---

# Description

Owner decision (2026-11-10, reply 346, 3:23 PM ET), (one consuming-project name redacted): "Insane. I hate tests that are real time sensitive. We fought this battle with [redacted]. Disable those tests and have an agent work to bring them back when they're robust. Each product lane must make its tests fast, high signal, and robust to running on a hot machine (>75% cpu on 316 HW threads). AND there should be good test selection so we are not running tests for agent-util A when only B is modified. Anything else should be disabled until we can improve it."

The five gate-exclusive cross differentials (`cross.agentctl.differential-0`, `cross.agentctl.differential-1`, `cross.agentctl.differential-2`, `cross.herdr-run.differential`, `cross.agentctl.herdr-agent-differential`) are disabled by default in `scripts/validate.py` under this decision: they are selected by no default, `--all`, or `--cross-all` run, so the gate-exclusive phase is empty. Each step stays runnable on demand with the explicit opt-in `--cross-differentials` flag, and directly via `python3 cross/differential.py --tool ...`.

A separate lane owns bringing them back robust; this issue tracks that work. It supersedes the exclusive-phase-removal approach for these steps: do not re-enable them by default until the acceptance criteria below pass.

# Design

- Remove or replace every wall-clock budget assertion in the five differentials (probe budgets as small as 50 ms judged beside 5 s / 10 s tool bounds) with load-aware or relative comparisons that still fail on a genuine regression: judge a measured time against host load or against a calibration measured in the same run, never by widening a budget until it cannot fail.
- Keep test selection component-scoped: a change to one paired tool must not select another tool's differential (the existing component filter is the starting point; audit it as part of this work).
- Keep each differential fast and high-signal on a hot machine: pass reliably above 75% CPU on a 316-hardware-thread host, with run time small beside the main validation phase.
- Re-enable by default only behind the recorded evidence below; keep the `--cross-differentials` opt-in and a way to return to disabled-by-default without a code change if the evidence regresses.

# Acceptance Criteria

- No wall-clock budget assertion remains in the five differentials, or each remaining one is justified in code by a same-run calibration that cannot mask a genuine slowdown (an injected slowdown of the measured path still fails under load).
- At least 20 consecutive main-phase (shared, non-exclusive) runs of each differential pass on a host above 75% CPU, with durations recorded in this issue.
- Component-scoped selection is demonstrated: a change touching only one paired tool selects only that tool's differential in the validation plan.
- The steps are re-enabled by default (selected by path selection and by `--all` as before the disable) only after the two criteria above are recorded here.

# Notes

Filed by mu-gatefast in the same commit that disables the five steps, at the owner's direct instruction (reply 346, 2026-11-10). Related: #1067 cross-differential-load-aware (the earlier load-aware approach, now carried by this issue's bring-back work).
