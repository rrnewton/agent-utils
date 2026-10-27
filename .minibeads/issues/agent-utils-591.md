---
title: 'headless-takeover: agentctl takeover and handback between headless and interactive'
status: open
priority: 2
issue_type: feature
labels:
- agentctl
- headless
depends_on:
  agent-utils-565: parent-child
created_at: 2026-10-27T17:37:14.509245390+00:00
updated_at: 2026-10-27T17:37:14.509245390+00:00
---

# Description

[opus 5.5] Child of #565 headless-coordinator (owner, 2026-10-27). agentctl takeover NAME stops the headless wrapper cleanly (finishing or interrupting the current turn) and opens an interactive Claude Code in a Herdr tab on the same session (claude --resume ID); agentctl handback NAME closes that interactive session and restarts the wrapper on the same session. There must never be two writers on one session.

# Acceptance Criteria

Live against a test agent: the interactive session sees the headless history, and the wrapper sees the interactive turns after handback; the wrapper refuses to start while a takeover holds the session.
