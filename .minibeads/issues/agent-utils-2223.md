---
title: 'vibe-talk-screenshots-walk-fixed-wait: the screenshots walk waits on its real condition, with a hang backstop'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- flaky-test
- hot-machine
created_at: 2026-11-24T15:51:00-05:00
updated_at: 2026-11-24T15:51:00-05:00
---

# Description

The screenshots walk (`vibe-talk.shots.screenshots-desks`, and its phones and
wide siblings) still waits with a fixed 30-second `wait_for_function`, where
the browser checks already wait on a condition with a generous hang backstop
(`vibe-talk/tests/browser_waits.py`, `HANG_BACKSTOP_MS = 120_000`).

Evidence (mu-flakeaudit, 2026-11-24): the step was red on 137 gate rows over
133 heads in 7 days, the same head was also green on 31 of those heads, and
the one retained failing log ends with the walk never reaching state
93-glossary-orphan ("Page.wait_for_function: Timeout 30000ms exceeded"),
after an intermittent `TargetClosedError`. Run alone 5 times on current
main at host load 98-207, the step passed 5/5 (173-309 s each). The check
is sound alone and fails only when a full gate starves it of CPU, which is
the owner's hot-machine rule (rule 346): no wall-clock-sensitive gating
checks.

# Design

- The walk waits on the observable page state it actually needs (the same
  condition the 30-second wait was proxying), never on a duration.
- The only time limit left is a hang backstop in the browser_waits sense:
  generous, and its failure message names the state that never arrived.
- The desks, phones and wide walks share the change where they share the code.

# Acceptance Criteria

- No fixed-duration wait remains in the screenshots walk; every wait names its condition.
- The three screenshot steps pass in a full gate on a hot host, and their #2175 known-red entries can drop.

# Notes

Filed from the frequent-red audit (ai_docs/reports/measurements/2026-11-24-frequent-red-gate-steps.md). The step is interim known-red under #2175 until this lands.
