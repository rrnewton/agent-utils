---
title: 'wrkslots-orphan-retire: a slot whose owner died and whose agent was never registered can never be removed'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-10-15T19:55:46.624149558+00:00
updated_at: 2026-10-15T19:55:46.624149558+00:00
---

# Description

[opus 5.5] In dev-agent-utils, slot wrkviz-transcript-container (agent hold-wrkviz-transcript-container, task wrkviz-transcript-container, generation 1) has a dead recorded owner (PID 2838589 not live, heartbeat expired) but can never be removed. bin/agent-liveness fails closed with no-agentctl-record: the agent name has no agentctl record, so the check returns unverifiable (rc 2), and wrkslots remove requires registered liveness rc 0. wrkslots doctor reports dead-owner; status shows owner=dead, yet removal is still blocked. Fix: accept proven owner death plus coordinator evidence (for example the recover-unbound-owner path) for unregistered agents, without weakening the fail-closed rule for agents that were registered and might still live. Acceptance: (1) a unit/integration test covers dead-owner + unregistered-agent removal succeeding only with the extra coordinator evidence; (2) a registered-but-unverifiable agent still refuses; (3) wrkviz-transcript-container can then be removed in dev-agent-utils and its work (7431ceef, also on GitHub as archived/wrkviz-transcript-container) is preserved. Design notes may be in ~/agent_scratch/ci-and-gap/.
