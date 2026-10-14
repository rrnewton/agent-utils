---
title: 'voice-post-prefix: a configurable tag on every message the voice agent posts'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-14T16:42:44.612016485+00:00
updated_at: 2026-10-14T18:47:05.459559053+00:00
closed_at: 2026-10-14T18:47:05.459558883+00:00
---

# Description

Owner 2026-10-14: messages the voice agent posts (to Google Chat, Slack or any provider) carry no sign that the voice agent relayed them. Add a prefix the owner can change in Settings (Voice agent group, Sending section), stored on the server like the #272 voice-agent-instructions so every device and call uses the same one. Default: '(Voice-agent relayed, from user) ' (with the trailing space). It is prepended to every post the voice agent makes: a spoken yes or a tap on the send card (#264/#269), a send with approval off, a reply into a thread, and every part of a split post (#263: prefix on the first part only). Posts the owner types on the page never get it. An empty prefix turns it off. The read-back before a send and the frozen transcript card (#270) show the text with the prefix, so what is read back is what is posted; the prefix counts toward the provider's post-size limit.
