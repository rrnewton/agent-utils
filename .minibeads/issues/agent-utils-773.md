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

# Notes

## Resolution note (2026-11-02)

[opus 5.5] The refusal was the sender's team-level may_send_to list, not a route defect: the agent-utils declaration on that host did not list the other team. Owner policy since 12:17 ET: cross-team traffic only between the two leads on one host, so that host's declarations now allow the pair and the other host's declarations restrict each team to itself (verified there: agent-utils -> other team refused with "may-send-to (team agent-utils may not send to <team>)"). Host-scoped allow entries are #779 agent-teams-host-scoped-allow. The rename-journal part moved to #774 (under investigation: current builds skip the layout marker; likely a stale caller build).
