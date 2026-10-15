---
title: 'slack-reactions: show the reactions Slack history reports on each message'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-15T04:34:05.345820882+00:00
updated_at: 2026-10-15T05:00:00.308910805+00:00
closed_at: 2026-10-15T05:00:00.308910705+00:00
---

# Description

Slack's conversations.history and conversations.replies return each message's reactions as reactions: [{name, count, users}], but vibe-talk's Slack client sets no reactions on the messages it reads (vibe-talk/src/slack/client.rs, the message conversion sets reactions: None), so the page shows none for Slack while it shows them for providers that report them (#219 emoji-reactions). Map Slack's reactions into the provider-neutral reaction list the page already draws (emoji name to the character where Slack's name is a standard shortcode, else the :name: text; count; whether the owner reacted, from users containing the owner's id), for history, Flat and thread reads and for live updates; keep an absent field as 'not reported', not as 'no reactions'. Test against tests/slack_fake with reactions on a root, a reply and an edited message. A compatible Slack bridge that cannot supply reactions keeps today's behaviour.
