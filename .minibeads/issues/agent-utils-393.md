---
title: 'watch-agents-background-idle: an agent waiting on its own background shell or sub-agent is reported idle without a verdict'
status: closed
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-10-20T07:47:56.373323759+00:00
updated_at: 2026-10-20T07:53:17.954010002+00:00
closed_at: 2026-10-20T07:53:17.954009832+00:00
---

# Description

[opus 5.5] watch-agents (#381 agent-watch) reported a Claude Code agent as idle for over 15 minutes without a verdict while it waited on its own background shell (a validation gate) or a background sub-agent; twice on 2026-10-19, both false. Claude Code shows such work in its footer (2 shells, ← 1 agent) and in the row that closes a turn (2 shells still running).

# Acceptance Criteria

While the bottom of the screen shows running background work, an idle or done agent is held to the stalled rule (timed from the last screen change) instead of idle-no-verdict; fixture tests from real Claude Code pane text.
