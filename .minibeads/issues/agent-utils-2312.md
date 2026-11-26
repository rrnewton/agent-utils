---
title: 'herdr-run-first-use-panebusy-zero-window: a just-created pane can never pass the idle judgement'
status: open
priority: 2
issue_type: bug
labels:
- herdr-run
created_at: 2026-11-26T16:24:15-05:00
updated_at: 2026-11-26T16:24:15-05:00
---

# Description

A lane's first `herdr-run` invocation in its slot created a fresh pane and then refused its own command with PANEBUSY (rc 75): the pane "looked idle but not for 2 consecutive checks within 0s", with the prompt judged `abstain` because the pane had no rendered output yet. A re-run after the pane had rendered passed. The idle judgement is given a 0 s window on a just-created pane, so a first use can never pass it.

The code path (Rust edition, primary): command-line defaulting takes `ready_timeout` from the configured `ready_timeout_seconds`, whose default is 0 when neither the flag nor configuration sets it. Target resolution records what it created on the target. Execution then spends that budget acquiring the pane lock, computes the remaining readiness budget as `(ready_timeout - elapsed)` clamped at zero, and judges readiness by polling for two consecutive ready samples. The prompt judge abstains while the pane has no rendered output, so a fresh pane needs render time plus two samples inside a window that is already spent.

# Design

On a pane this invocation itself just created (the resolution result says so), wait a bounded time for the first prompt to render before judging idleness: the wait's condition is the first rendered prompt, with a generous backstop, and no wall-clock verdict. Panes the invocation did not create keep today's judgement unchanged. The Python edition mirrors the same defaulting and judgement and should take the same fix.

# Acceptance Criteria

- A first `herdr-run` use that creates its pane runs the command instead of refusing PANEBUSY, with a test whose verdict does not depend on wall-clock timing.
- A pane that never renders still refuses, bounded, with the journal of what was judged.
- Existing busy-pane refusals for panes the invocation did not create are unchanged.

# Notes

Reported from a consuming lane and trusted; filed by the coordinator. The 0 s window also arises whenever lock acquisition consumes the whole readiness budget, so the fix should judge from the remaining budget's cause, not only from the configured default.
