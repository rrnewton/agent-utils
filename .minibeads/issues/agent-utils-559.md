---
title: 'claude-shell-mode-prompt: a prompt starting with ! is left unsent in a Claude Code composer and quarantined'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- submission
created_at: 2026-10-26T21:58:51.802964434+00:00
updated_at: 2026-10-26T21:58:51.802964434+00:00
---

# Description

[opus 5.5] Found while reproducing #556 chat-delivery-latency on a test pane (Claude Code 2.1.310, fullscreen TUI). A queued prompt whose text starts with '!' switches Claude Code's composer to shell mode, which redraws '!echo x' as '! echo x' with a '! for shell mode' footer. The verified submission never sees the exact text staged, so it sends no Enter and, after the 60 s stage timeout, the drain quarantines the item as possibly_submitted (82 s in one run), leaving the text unsent in the composer, where it blocks the next delivery until it is cleared. Seen at the bottom and when scrolled up, so scroll position is not the cause. Bridge prompts never start with '!', but an agentctl send of raw text can, and that text would run as a shell command in the agent's pane if it were submitted.

# Acceptance Criteria

Either refuse to type a prompt that starts with '!' into Claude Code (NotStaged, with a clear reason; nothing typed), or recognise the shell-mode rendering as staged. Decide whether running a sender's text as a shell command is acceptable at all. Fixture test from a live capture.
