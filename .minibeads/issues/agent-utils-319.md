---
title: 'slack-reply-by-id: reading a Slack thread reply by its id answers unknown_message'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-15T06:03:00.400273191+00:00
updated_at: 2026-10-15T06:03:00.400273191+00:00
---

# Description

Found by a read-only exercise of a Slack deployment on 2026-10-14 (the Slack move's parity stage). A Slack thread reply that GET /messages, the digest, the todo list and activity all list cannot be read by id: GET /messages/{id} (and the voice agent's read_message) answers 404 unknown_message, because for Slack the by-id lookup reads only conversations.history, which holds no replies. It works with ?thread_id=, but digest entries carry no thread id and read_message passes none, so the voice agent can be told about a reply and then fail to read it. Make a by-id read of a Slack reply work without a thread id (use the thread the store or cache already knows for that id, or the reply's thread_ts from the listing that surfaced it), and include the thread id in digest/todo/activity entries. Tests against tests/slack_fake and the MCP read_message tool.
