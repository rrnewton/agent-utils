---
title: 'tick-hub-task-health: a tick-hub configuration that watches the trackers for this harness'
status: open
priority: 2
issue_type: task
labels:
- tasks
- tick-hub
depends_on:
  agent-utils-634: blocks
  agent-utils-631: parent-child
  agent-utils-632: blocks
created_at: 2026-10-28T13:57:45.667730517+00:00
updated_at: 2026-10-28T13:57:49.479419064+00:00
---

# Description

[opus 5.5] Part of #631 task-tracker-adapters (near-term fix 4). tick-hub can do this today with shell gates and captured key=value fields; no tick-hub change is needed.

Wanted: an ops.yaml for this harness (in the harness, not this repository; a generic example in skills/agent-utils-setup/templates/tick-hub/ if it reads well), run by the usual tick-deliver timer, with reminders that emit ACTION: lines to the coordinator when:
1. in_progress has an issue whose assignee is not a running agent (agentctl list --json), or a running agent has no in_progress issue (an unclaimed dispatch);
2. an issue became ready since the last tick (mb ready, diffed against the previous tick's list);
3. open issues grew by more than N in a day, or an open issue has not been updated in 14 days;
4. a GitHub issue has been open more than 3 days without the 'triaged' label (one paced gh call per tick at most; hourly cadence).
Each gate prints counts and up to five ids; thresholds are fields in the YAML.

Blocked by #632 task-dispatch-protocol (check 1 needs claims to exist) and #634 github-triage-pass (check 4 needs the label convention).
