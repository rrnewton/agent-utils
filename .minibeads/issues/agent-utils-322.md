---
title: 'slack-quote-lines: a Slack quote swallows the line after it'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-15T06:03:00.411080221+00:00
updated_at: 2026-10-15T06:03:00.411080221+00:00
---

# Description

Found by a read-only exercise of a Slack deployment on 2026-10-14 (the Slack move's parity stage). In Slack mrkdwn a '>' quotes only its own line, but vibe-talk's Markdown conversion lets the next unquoted line join the quote, so the owner's answer under a quoted question is drawn inside the quote (5 messages in one channel). End a quoted run at the first line without '>' (for example by emitting a blank line in the mrkdwn-to-Markdown conversion), keeping multi-line quotes that repeat '>' intact. Tests: one quoted line then an answer, several quoted lines, a quote at the end.
