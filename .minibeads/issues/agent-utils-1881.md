---
title: 'vibe-talk-own-messages-read-on-arrival: own messages are read on arrival in every view'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- read-state
created_at: 2026-11-18T20:53:00-05:00
updated_at: 2026-11-18T20:53:00-05:00
---

# Description

[watermelon-full-preview] Owner report, thread 208 (2026-11-18): "I was just in this main session and all MY messages showed as UNREAD. I went to check my settings for this channel, confirming them, came back and THEN they were all marked read!?!? Is there just some big delay there?" The server had restarted for a deploy shortly before. Follow-up, thread 210: "I send a message, it's mine, as soon as it's a card it should already show as marked read. This is relevant when I press shift up to go to the prev unread message.. I don't want to scroll through my own messages."

The rule (#549 own-read-stored): the owner's own messages (sender = the owner's user id, the coding_agent/owner identity in config) are READ on arrival, in all views. Today they show unread until some later pass — here, the settings round-trip triggered one.

# Design

- Server: an own message's read bit is decided when the server ingests it (live publish, poll/backfill fetch, cache ingest, post through this server), not only when a later read happens to meet it; decided once, never re-applied, as #549 holds.
- Client: the owner's own card is READ from the optimistic copy onward (no wait for the server); the Slack echo (new id/ts replacing the optimistic copy) keeps it read; a replayed or backfilled copy keeps it read; a history page fold marks own messages it carries.
- Unread navigation (Shift+Up/Down), unread counts and Newest(N) never stop on or count own messages.
- Views: Main, All, thread and two-column, after send + echo, after a server restart or reconnect, and after a page reload.

# Acceptance Criteria

- By hand on a real test server: send, wait for the echo, restart the server, reload; own messages are never unread in Main, All, thread and two-column views.
- A cheap unit/integration test pins the server-side ingest rule.
- A cheap page test pins the optimistic/echo read state and unread navigation skipping own messages.
- Screenshot matrix per the standing owner-UI acceptance (light/dark, keyboard open/closed, phone/desktop, Discuss open on phone, before/after of the targeted state) with BUILD.txt, via the shared harness.

# Notes

May share a root with #1880's echo replacement (mu-readerdrill owns that lane); coordinate before touching echo-replace code.
