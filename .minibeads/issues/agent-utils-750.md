---
title: 'send-urgent-drains-backlog: an urgent send must not drain the whole pending inbox into a busy agent'
status: open
priority: 3
issue_type: task
labels:
- agentctl
created_at: 2026-11-02T16:26:35.715936567+00:00
updated_at: 2026-11-02T16:26:35.715936567+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). 'agentctl send --urgent' drained the target's whole pending inbox (~13 items queued since ~11:00 ET: subagent done-notes, chat requests, timer reminders) into a busy Muse coordinator in one burst before its own message (2026-10-29 13:22 ET, a consumer team a coordinator). Want: urgent delivers only its own message (jump the queue) or asks; also a queue-age warning in status. File as issue.
