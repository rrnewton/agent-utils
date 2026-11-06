---
title: 'cloud-coordinator-reminders: reply reminders and idle detection for agentcloud-session coordinators come from session history'
status: closed
priority: 2
issue_type: feature
assignee: cl-start
created_at: 2026-11-05T17:42:01.282299906+00:00
updated_at: 2026-11-06T14:45:00.000000000+00:00
closed_at: 2026-11-06T14:45:00.000000000+00:00
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

## Progress (cl-start, 2026-11-06)

[opus 5.5] Cause: the chat bridge reads a cloud coordinator's pane facts from `cloud_pane_info`, which reported no harness (`agent: None`) and took its status from the lagged `agentcloudctl list`. Reminders then named "an unrecognized harness" and never reminded it.

Change:
- `cloud_pane_info` now names the harness `agentcloud` and reads the turn from the session's own journal: `agentcloudctl history --raw --type run_started,run_finished,user_input,input_applied --last 200`.
  - `working`: a `run_started` has no `run_finished` for its run.
  - `waiting`: an input journaled after the last finished run is not applied yet.
  - `idle`: otherwise.
  - `unknown`: the history cannot be read. Nothing is guessed from the pane.
- Reply reminders treat `agentcloud` like Codex and Muse: idle only when that status says idle.
- On `unknown`, reminders log that they wait because the history is unreadable, and the coordinator is not reminded.
- The reply method was already right: since #896, replies come from the session history, so the prompt keeps its fenced reply block.

Tests:
- The turn read from recorded run and input events.
- The cloud pane facts, including `unknown` with history failing, and that the history call filters by those event types.
- Reminders for an agentcloud coordinator: named agentcloud, idle, working, and unknown (waits, never "unrecognized").

Not changed: `agentctl status` for a cloud agent still uses the fleet listing.

Remaining: the live check through the lead.

## Resolution

Landed in de02b9f3's stack: a cloud coordinator's pane facts name agentcloud and read working, waiting or idle from its session history (unknown when unreadable); reminders wait on unknown and never say unrecognized. Gate: the full gate passed on the pre-rebase stack (f1226d35 on a7087281). The rebase onto ab1d7b16 overlapped agent_watch.rs and failed_digest.rs; targeted agent_watch, failed_digest and revive tests (202) and clippy passed on the rebased tree.
