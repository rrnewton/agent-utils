---
title: 'parent-child-responsiveness: measure and shorten the time a coordinator takes to answer its children'
status: open
priority: 2
issue_type: task
labels:
- agentctl
- wrkviz
created_at: 2026-10-25T13:03:32.087053397+00:00
updated_at: 2026-10-25T13:03:32.087053397+00:00
---

# Description

[opus 5.5] How promptly does an agentctl coordinator learn that a child stopped, and act? Measured on one 5.6-day agentctl team (76 children, Claude Code and Muse; 142 child turn ends the parent had to answer) with a new zero-model `wrkviz responsiveness` report.

Findings (figures from `wrkviz responsiveness`, 2026-10-19 to 2026-10-25):
- The coordinator was busy only 9.5% of the time, yet the median time from a child's turn end to the coordinator acting on it was 9.8 min (p90 28.4).
- 80 of 134 responses came through the coordinator's periodic sweep prompt (a session cron, every 20 then 30 min): p50 18.6 min, p90 31.8. When the child itself sent `agentctl send coordinator`, the parent acted in 1.0 min p50; queue delivery to the coordinator took 1.1 min p50.
- A DONE: reply is not a notification. agentctl watch-agents deliberately ignores idle agents that show a verdict, and most children end with DONE: on screen without messaging the parent, so DONE episodes waited 11.3 min p50 for the sweep.
- An idle coordinator stayed idle 9.1 min p50 after a child turn end; an Orc coordinator's wakeup loop (2 s agent poll, 5 s idle cooldown, wakeup only between parent turns) starts a parent turn 0.3 to 0.5 min p50 after a child report.
- 31.7% of child idle time was spent waiting on the parent (19.4 h answering, 11.6 h DONE until stopped), against 67.0 h waiting on the children's own background work.

Delivered here: `wrkviz responsiveness` (py/wrkviz/responsiveness.py) and its user-guide section.

Follow-ups worth separate issues:
- agentctl watch-agents: a `child-done` notice the moment a DONE:/BLOCKED: verdict appears (no threshold), batched and routed to the nearest managed_by parent.
- wrkviz: an agentctl provider that links registry children to the coordinator team, so the site shows the whole tree, plus a responsiveness view.

# Acceptance Criteria

A zero-model report of child-to-parent latency and idle time from agentctl records; findings and recommendations recorded.
