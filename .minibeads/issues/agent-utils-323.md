---
title: 'slack-system-bots: a Slack system bot is drawn as the coding agent'
status: closed
priority: 3
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-15T06:03:00.414239690+00:00
updated_at: 2026-10-15T08:01:48.063297185+00:00
closed_at: 2026-10-15T08:01:48.063296695+00:00
---

# Description

Found by a read-only exercise of a Slack deployment on 2026-10-14 (the Slack move's parity stage). Slack system bots (for example a workspace migration's welcome bot) are drawn as the coding agent: no name, no '(bot)' label, agent styling, because the page assumes the only non-self bot in a channel is the coder. Draw system bots with their own name and a bot label, and pick the coder only from bots the configuration or the channel's agent settings name. Settings can still override.
