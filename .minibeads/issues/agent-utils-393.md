---
title: 'watch-agents-background-idle: an agent waiting on its own background shell or sub-agent is reported idle without a verdict'
status: closed
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-10-20T07:47:56.373323759+00:00
updated_at: 2026-10-20T08:02:40.177672498+00:00
closed_at: 2026-10-20T07:53:17.954009832+00:00
---

# Description

[opus 5.5] watch-agents (#381 agent-watch) reported a Claude Code agent as idle for over 15 minutes without a verdict while it waited on its own background shell (a validation gate) or a background sub-agent; twice on 2026-10-19, both false. Claude Code shows such work in its footer (2 shells, ← 1 agent) and in the row that closes a turn (2 shells still running).

[opus 5.5] Extended after a third false report on 2026-10-20 ~01:10, against an agent that was working while herdr said idle (footer: esc to interrupt, output streaming): Claude Code's footer segment 'esc to interrupt' also counts as work running, idle-no-verdict is now timed from the last screen change, and when the input box is in view only the footer under it is read, because the turn's closing row keeps saying 'still running' after the work ends. Known residual: an agent with a perpetual background shell (a dev server, tail -f) is never reported idle-no-verdict, only stalled once its screen holds still for --stall-minutes.

# Acceptance Criteria

While the bottom of the screen shows running background work, an idle or done agent is held to the stalled rule (timed from the last screen change) instead of idle-no-verdict; fixture tests from real Claude Code pane text.
