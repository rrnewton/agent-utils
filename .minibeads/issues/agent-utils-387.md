---
title: 'discord-file-posts: a Discord post that is only a file or an embed reaches the page as a line naming it'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-19T22:18:47.668393132+00:00
updated_at: 2026-10-25T21:47:59.469412276+00:00
closed_at: 2026-10-25T21:47:59.469412225+00:00
---

# Description

[GPT-6 Codex codex-vibe-discord] Discord attachment/image/embed-only posts had empty content and were hidden by hide-empty-messages (agent-utils-386). Confirmed the fix was absent from origin/main at 9fa6d01a. Adapt attachments and embeds into the existing bounded, escaped Slack file-line formatter; retain names, kinds and safe links without fetching files. Add Discord-shaped JSON tests, malformed-link/name escaping control, a truly-empty control, and a client test with the default empty-message filter enabled. Focused server/client tests pass. Mutation removing attachments and embeds fails the new server regression. Full selected repository validation running before publication.

# Acceptance Criteria

A Discord message with attachments carries one line per attachment in its body, after its words when it has any, naming the file and its kind as #313 slack-file-posts does for Slack, and an embed-only message carries its title or link; nothing fetches a file. Such a message is then not empty for #386 hide-empty-messages and is drawn, counted and read. Server tests against Discord-shaped JSON, and a page test that it is shown with the switch on.
