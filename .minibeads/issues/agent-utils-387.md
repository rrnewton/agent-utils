---
title: 'discord-file-posts: a Discord post that is only a file or an embed reaches the page as a line naming it'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-19T22:18:47.668393132+00:00
updated_at: 2026-10-25T22:12:05.059870903+00:00
closed_at: 2026-10-25T21:47:59.469412225+00:00
---

# Description

[GPT-6 Codex codex-vibe-discord] Fixed Discord attachment/image/embed-only posts hidden by hide-empty-messages (agent-utils-386). The fix was absent from origin/main at 9fa6d01a. Discord attachment/embed fields now reuse the existing bounded, escaped file-line formatter, retaining names, kinds and safe links without fetching files. Discord-shaped JSON and default-filter client regressions pass; removing attachments/embeds fails the server regression. Verification: 2,338 JavaScript tests and the full Rust recheck (2,737 passed, zero failed across 90 binaries), plus the selected browser checks. Aggregate gate failures and their passing focused rechecks are recorded in the coordinator progress artifact.

# Acceptance Criteria

A Discord message with attachments carries one line per attachment in its body, after its words when it has any, naming the file and its kind as #313 slack-file-posts does for Slack, and an embed-only message carries its title or link; nothing fetches a file. Such a message is then not empty for #386 hide-empty-messages and is drawn, counted and read. Server tests against Discord-shaped JSON, and a page test that it is shown with the switch on.
