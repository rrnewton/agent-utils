---
title: 'cloud-coordinator-reminders: reply reminders and idle detection for agentcloud-session coordinators come from session history'
status: open
priority: 2
issue_type: feature
assignee: cl-start
created_at: 2026-11-05T17:42:01.282299906+00:00
updated_at: 2026-11-05T17:42:01.282299906+00:00
---

# Description

A coordinator running in an agentcloud session gets reply reminders and idle/working detection that fit pane agents, not cloud sessions. On 2026-11-05 a consumer team's cloud coordinator still received reminders that called its harness "unrecognized", even after #852 C. Its idle state is read from a pane view rather than from the session itself.

# Design

- For an agentcloud-session coordinator, derive reply reminders and idle/working state from `agentcloudctl history`: run started and finished events, settled model calls, and user_input applied. That is the same evidence source #896 uses for receipts.
- Reminders name the right harness and the right reply method: under the #896 policy, the command only when no transcript is followed.
- Where history is unavailable, say so in status rather than guessing idle.

# Acceptance Criteria

- Test with recorded history: a finished run reads idle and a running one reads working. A reminder for an unanswered request names agentcloud, never "unrecognized harness".
- Test: with history unreachable, status says unknown, and no idle notice is raised from a pane guess.
- A live check through the lead: the consumer team's cloud coordinator gets a correct reminder.

# Notes

Requested by the lead on 2026-11-05 at 06:34 AM ET (P2, cl-start after #884 and the reply-policy work, now folded into #896). The request was lost in the #884 pane-lock window (the coordinator's failed/ queue, possibly_submitted), then recovered and filed at 12:50 PM ET. Assigned to cl-start after #896 and #897. Related: #852 D (an agentcloud record through its agentterm view) and #900.
