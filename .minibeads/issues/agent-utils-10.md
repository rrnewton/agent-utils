---
title: 'agent-space-linking: support or document one-space-per-agent linking'
status: in_progress
priority: 2
issue_type: task
assignee: opus-5.5
labels:
- agentctl
- chat
- design
depends_on:
  agent-utils-1: parent-child
created_at: 2026-09-25T01:32:03.478573636+00:00
updated_at: 2026-09-25T04:48:21.466739184+00:00
claimed_at: 2026-09-25T04:48:21.466739184+00:00
claimed_until: 2026-09-27T04:48:21.466611662+00:00
---

# Description

[gpt-5.6-sol] Determine whether one managed agent can be linked directly to its own chat space instead of requiring a thread in a shared space, then implement or document the supported mapping model.

# Acceptance Criteria

Capabilities and constraints are documented from real bridge behavior; either a per-agent space can be linked and verified end to end, or the exact blocker and best supported alternative are recorded.
