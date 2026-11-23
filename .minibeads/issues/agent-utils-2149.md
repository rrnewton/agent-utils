---
title: 'agentctl-watch-non-agent-dirs-as-agents: agentctl watch treats any directory under the agents directory as an agent'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-23T06:15:00-05:00
updated_at: 2026-11-23T06:15:00-05:00
---

# Description

Since the morning turnover, agentctl watch reports 'hooks_trust hooks: cannot read JSON .../agents/hooks/agent.json' on two hosts. A registry keeps a non-agent directory under its agents directory (a hooks directory holding one hook script); the watch enumerates every directory there as an agent and alarms on the missing record.

# Design

Enumerate agents only from directories that hold an agent record (agent.json), or from the registry index; skip other directories silently (optionally note them once at debug level). Never alarm on a directory that never was an agent.

# Acceptance Criteria

- Test: a registry with an extra non-agent directory under agents produces no watch problem; a real agent directory with a missing or broken agent.json still does.

# Notes

Reported by a consuming team after the morning services turnover; filed by the coordinator.
