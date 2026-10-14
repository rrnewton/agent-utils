---
title: 'slack-reply-live: Slack thread replies arrive live and reach the voice tools'' recent windows'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-14T20:24:13.547842824+00:00
updated_at: 2026-10-14T20:45:49.158810147+00:00
closed_at: 2026-10-14T20:45:49.158810006+00:00
---

# Description

Slack move plan item 1.5 (2026-10-14). The Slack live poller and fetch_recent read conversations.history only, so a thread reply never arrived live and never reached the voice tools' recent windows. Each poll now also reads replies, only for roots whose latest_reply moved, from the newest one held (one conversations.replies call per moved root, none for quiet ones), bounded per poll with carry-over, so one failing thread holds up no other; the clear paths keep reading top-level history only.
