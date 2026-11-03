---
title: 'restore-write-agents-trusts-stale-lifecycle: restore --write-agents must not drop agents that are alive'
status: closed
priority: 1
issue_type: bug
assignee: mu-restore@devbig030
labels:
- agentctl
created_at: 2026-11-03T08:45:50.739371834+00:00
updated_at: 2026-11-03T09:36:47.847195230+00:00
closed_at: 2026-11-03T09:36:47.847195099+00:00
---

# Description

[opus 5.5] Filed for the devbig014 coordinator (P1). `agentctl restore --write-agents` (f25c915f, #742) builds the desired agents list from each record's lifecycle and ignores live state. On a consumer team's host its preview would have REMOVED two live Codex lanes (working, mid-goal for days) because their records still said launch_failed; applying it would have silently dropped live lanes from reboot restore.

# Design

- Decide membership from live state as well as the record: the Herdr pane exists, the recorded harness process is alive, the harness is in the foreground. Never drop a record whose pane or process is alive.
- The preview flags every record/live mismatch (record says launch_failed or stopped but the pane or process is alive, and the reverse), naming the evidence.
- `--apply` refuses while mismatches are unresolved, saying how to resolve them (repair the record, or name the agent explicitly to keep or drop).

# Acceptance Criteria

- A record with lifecycle launch_failed whose pane and process are alive stays in the list, and the preview reports the mismatch.
- `--apply` refuses while any mismatch is unresolved; tests cover the refusal and the resolution path.
- A genuinely dead record is still removed.
