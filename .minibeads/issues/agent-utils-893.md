---
title: 'muse-stop-review-hook: agentctl stop of a Muse agent accounts for its lingering session-review hook'
status: open
priority: 3
issue_type: bug
assignee: cl-start
created_at: 2026-11-05T15:37:16.448037631+00:00
updated_at: 2026-11-05T15:37:16.448037631+00:00
---

# Description

After `agentctl stop` of a Muse agent, a `muse-session-review-hook --wait muse <session-id>` process lingers with its working directory in the agent's slot. It was seen alive more than 45 minutes after a stop, for two lanes on 2026-11-05. `wrkslots remove` then refuses with "live process uses slot … muse-session-re".

# Design

Pick one, and document the behaviour in the agentctl skill and userguide either way:
- `agentctl stop` for a Muse agent waits, bounded, for its session-review hook to exit, and reports it if it is still running ("session review hook still running, pid N; slot removal will wait for it").
- Or `wrkslots remove` recognises the hook as a known post-session process and names it with a remedy (wait, or how to end it safely), instead of the generic live-process refusal.

At minimum, the agentctl skill documents the wait.

# Acceptance Criteria

- Stopping a Muse agent either waits for, or clearly reports, the lingering hook.
- `wrkslots remove`'s refusal names the hook and the remedy.
- The documentation covers the wait.

# Notes

Reported by the lead on 2026-11-05 (P3). Assigned to cl-start's backlog.
