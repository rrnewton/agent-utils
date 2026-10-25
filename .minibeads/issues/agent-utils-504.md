---
title: 'agent-teams-core: build the agent-teams tool (P1 model, P3 exactly-once core, P4 ssh route)'
status: open
priority: 2
issue_type: feature
labels:
- agent-teams
created_at: 2026-10-25T12:32:16.217755995+00:00
updated_at: 2026-10-25T12:32:16.217755995+00:00
---

# Description

[muse watermelon] Build rs/agent-teams per ai_docs/transient/2026-10-24-agentctl-team-messaging.md revision 3: P1 read-only model (teams.json, routes.d with git-ignored *.private.json and AGENT_TEAMS_ROUTES_PATH, declaration.json, show/probe/check/import); P3 exactly-once core (ids, outbox, ledger claim/fence/recovery, accept/receipt/fence/serve, send/status/pump/retract, sender proof with the managed_by tree, local route, Deliverer and Route traits, model-based harness of section 9.7 with make soak-agent-teams); P4 ssh route (use_user_ssh_config permanently, dedicated forced-command keys optional), on-demand and persistent links, star topology with one hub. Live trial between two hosts follows with the coordinator.

# Acceptance Criteria

P1 and P3 land green with the model harness in the gate; P4 ssh driver lands with tests; CLI quickstart/userguide/help and RELATED_WORK.md present; live trial recorded separately.
