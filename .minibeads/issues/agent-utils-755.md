---
title: 'agent-teams-stale-deliverer: a retired coordinator must not stay the team deliverer'
status: open
priority: 3
issue_type: task
labels:
- agent-teams
created_at: 2026-11-02T16:26:35.733429813+00:00
updated_at: 2026-11-02T16:26:35.733429813+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). a consumer team's team declaration on a host kept deliverer a retired coordinator after a retired coordinator was retired (10-29); 13 messages piled into a dead inbox while 'agent-teams coordinators' showed a coordinator live and senders saw 'accepted'. agent-teams should refuse or flag delivery to an archived/absent agent, and check should compare deliverer with the registry's designated coordinator. Also coord retirement runbook must re-run init --coordinator-self. (2026-11-01)
