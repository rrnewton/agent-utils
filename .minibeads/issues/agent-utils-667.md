---
title: 'subagent-backpressure: escalating warnings when a coordinator leaks agents'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
created_at: 2026-10-28T17:39:22.209245322+00:00
updated_at: 2026-10-28T17:39:22.209245322+00:00
---

# Description

[opus 5.5] owner 13:12 ET. Deterministic backpressure against leaked subagents: a configured target and maximum count of live agents per coordinator/registry; agentctl start prints escalating warnings (and past the max requires --force with a reason) when the coordinator has idle/finished agents it has not stopped; watch-agents and a tick-hub check report agents idle past N minutes with no open task, finished agents not retired, and tabs with no registry record (leaked Herdr tabs), with urgency growing with count and age.
