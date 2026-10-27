---
title: 'headless-watch: watch-agents status for headless agents from the event log'
status: open
priority: 2
issue_type: feature
labels:
- agentctl
- headless
depends_on:
  agent-utils-565: parent-child
created_at: 2026-10-27T17:37:08.213184635+00:00
updated_at: 2026-10-27T17:37:08.213184635+00:00
---

# Description

[opus 5.5] Child of #565 headless-coordinator. agentctl watch-agents skips headless (adapter: harness-wrapper) agents. Give them status from the event log and queue instead of a screen (design 7.2): idle/busy from the last status and turn events, turn age, queued inputs, and stuck states (a turn running past the stall threshold, inputs waiting while the wrapper is idle, an unreachable control socket, a dead wrapper), plus the idle-without-DONE:/BLOCKED: rule read from the turn's last assistant.text.

# Acceptance Criteria

watch-agents reports and notifies on a headless agent's stall, dead wrapper, and idle-without-verdict from its events; tests over scripted logs.
