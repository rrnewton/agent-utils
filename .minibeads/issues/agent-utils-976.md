---
title: 'workspace-hygiene: agentctl watch closes idle bare shells with no registered agent'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-06T20:49:36.084384729+00:00
updated_at: 2026-11-06T20:49:36.084384729+00:00
---

# Description

workspace-hygiene: agentctl watch closes idle bare shells with no registered agent

Owner request (reply 178). In every Herdr workspace agentctl manages,
agentctl watch (and restore/revive) closes any pane/tab that is a bare
shell (foreground = login shell, no child processes) with no registered
agent pointing at it, once idle past a grace period (~10 min,
configurable), logging every close (pane, tab, cwd, why) in health.json
/ the watch log. Never close a pane with a running program, a registered
agent's pane, or another workspace's panes; idempotent. Whether
hand-opened shells are fair game after the grace period is a setting
(default: on). Builds on item 5 slice 1's census and bare-shell closing;
coordinate with #961 watch timer repair.
