---
title: 'mark-own-messages-read: the owner''s own messages are never marked read'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- read-state
created_at: 2026-10-26T15:11:54.825085807+00:00
updated_at: 2026-10-26T18:23:09.616509318+00:00
closed_at: 2026-10-26T18:23:09.616509218+00:00
---

# Description

[opus 5.5] Owner, 2026-10-26, on the Slack instance: "the \"Mark my own messages as read\" feature is COMPLETELY broken. All my messages are still unread. In the Channels setting it appears to correctly have me/coding-agent labeled. They are rendering as MY messages... but they are not marked read."

Found: the setting was only ever a page-local display rule. A row whose author is the owner is greyed and left out of Hide read, but nothing records it as read: its Done button still offers Done (not Unarchive), the server holds no mark, and the server-side to-do list and every other device or consumer count it as unread. On the channel in question the owner had begun pressing Done on his own messages by hand.

Wanted: with the setting on (the default), every message from one of the owner's accounts is marked read in the same way Done marks it: messages that already exist are swept when a read meets them, and new ones are marked as they arrive. The owner's accounts are [owner] user_ids, each provider's owner_user_id, the accounts the synced who-is-who choice calls "me", and messages this server posted for him. An own message he puts back is not marked again.
