---
title: 'in-app-message-links: chat permalinks open the message in vibe-talk, and Back returns'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- ui
created_at: 2026-10-26T18:51:45.187487276+00:00
updated_at: 2026-10-26T18:51:45.187487276+00:00
---

# Description

[opus 5.5] The owner, 2026-10-26: threads that cite prior messages by permalink are horrible on mobile; each tap leaves the app for Slack. Recognize message permalinks of every configured provider (Slack https://WS.slack.com/archives/C/pTS with ?thread_ts for replies and slack:// app links; Google Chat chat.google.com/room/... and mail.google.com chat links; Discord discord.com/channels/G/C/M) in rendered bodies and, where the channel is one this server serves, draw them as in-app links that jump to the message (switching channel or view, loading it by id, opening its thread when it is a reply). Each jump pushes a history entry so the Android back gesture and the browser's Back return to the exact earlier spot; several jumps stack. In-app links are ON by default, with a setting to turn the rewriting off; the target's Open in Slack / Copy link still give the upstream address.

# Acceptance Criteria

Server tests per provider (thread replies, unknown shapes untouched, links in code untouched); page tests for jump, history stack and popstate restore; a Chromium walk at 393x852 touch and on a desk: tap a link in a long thread, land on the target, Back returns to the original scroll position.
