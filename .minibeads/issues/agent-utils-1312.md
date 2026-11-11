---
title: 'headless-cancel-queue: agentctl cancel refuses headless agents so a mistaken queued turn cannot be withdrawn'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-11T15:58:59.580190708+00:00
updated_at: 2026-11-11T15:58:59.580190708+00:00
---

# Description

[watermelon-full-preview] Follow-up discovered during #1311 console-follow (owner reply 370 session, coordb live evidence from devbig030): `agentctl cancel` REFUSES headless agents, so a turn queued by mistake from `agentctl console` (for example /help or a shell command typed into the pane before local command handling existed) cannot be withdrawn. With ~9 turns pending behind a running turn, an operator has no way to remove a mistaken queued turn.

# Design

- This is a SEPARATE follow-up to #1311 console-follow, not part of that fix. #1311 prevents mistaken queueing locally (/help and unknown /commands are refused locally); this issue covers withdrawing a turn already queued.
- Design to be decided in this issue: how cancel identifies a queued (not running) headless turn (queue id / position), what happens to positions behind it, and what the console and CLI report.

# Acceptance Criteria

- A queued headless turn can be cancelled by an identified handle and is removed without disturbing the running turn or other queued turns.
- Cancelling an unknown or already-running turn reports that clearly.
- Console/CLI output after cancellation reflects the new queue positions.

# Notes

Filed by mu-consolefix in the same issue-only tree as #1311 console-follow, as a separate follow-up (not part of the #1311 fix). Evidence: coordb live check on devbig030 during owner reply 370 session; `agentctl cancel` refuses headless agents.
