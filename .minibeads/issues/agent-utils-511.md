---
title: 'voice-explicit-send-only: send to the chat or coding agent only on an explicit user ask'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- voice
created_at: 2026-10-25T15:35:52.826572475+00:00
updated_at: 2026-10-25T16:18:27.828104261+00:00
closed_at: 2026-10-25T16:18:27.828104111+00:00
---

# Description

[opus 5.5] Owner report 2026-10-25 (driving): the voice agent posted a garbled speech fragment ("afflicting guidance and remind on") to a channel with confirmation off. Policy: the voice agent sends to the chat or the coding agent only when the user explicitly asks with an active verb addressed to it ("send this to", "tell the coding agent", "ask Claude"), whether or not confirm-before-send is on. Never pass an utterance through as a relay; a fragment, a question to the voice agent, or unclear speech is answered or clarified. Two layers: the prompt and tool descriptions say so, and the server refuses a post_reply whose user_request does not match an explicit directive in what the user actually said recently.

# Acceptance Criteria

Prompt and tool text state the rule in both send modes; post_reply requires user_request; the server checks it against recent user speech the page reports and refuses otherwise, content-free in the log; the screenshot case is refused and real positive phrasings pass; tests cover both.
