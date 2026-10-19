---
title: 'discord-file-posts: a Discord post that is only a file or an embed reaches the page as a line naming it'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-19T22:18:47.668393132+00:00
updated_at: 2026-10-19T22:18:47.668393132+00:00
---

# Description

[opus 5.5] Found while building #386 hide-empty-messages. The Discord adapter (src/discord/http.rs parse_message) reads a message's content and nothing of its attachments or embeds, so a Discord post that is only an image, a file or an embed reaches the page with empty text: it was drawn as a blank row, and with the Empty messages switch on (its default) it is now left out entirely. Slack files already arrive as a line naming the file (#313 slack-file-posts); Discord's do not.

# Acceptance Criteria

A Discord message with attachments carries one line per attachment in its body, after its words when it has any, naming the file and its kind as #313 slack-file-posts does for Slack, and an embed-only message carries its title or link; nothing fetches a file. Such a message is then not empty for #386 hide-empty-messages and is drawn, counted and read. Server tests against Discord-shaped JSON, and a page test that it is shown with the switch on.
