---
title: 'agent-teams-cross-team-route-refused: a coordinator''s sends to another team are refused on one host and allowed on another'
status: open
priority: 2
issue_type: task
labels:
- agent-teams
created_at: 2026-11-02T16:42:17.752544418+00:00
updated_at: 2026-11-02T16:42:17.752544418+00:00
---

# Description

[opus 5.5] 2026-11-02: agent-teams refuses sends from one host's agent-utils coordinator to a consumer team's coordinator on the same host, while the other host's agent-utils coordinator can send to it; messages had to be relayed through the other host. Make cross-team routes symmetric per the team declarations (accepts-from), and report the exact refusing rule. Related: on the same host that team's registry refused 'agentctl send coordinator' with 'unexpected entry in rename journal directory: .agentctl/.renames/upgrade-agentctl-registry-layout-2' (a migration leftover should not wedge the registry; agentctl doctor should offer the cleanup).
