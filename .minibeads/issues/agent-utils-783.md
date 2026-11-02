---
title: 'chat-reply-capture-miss: a well-formed reply block from an agent with no session binding was never captured'
status: open
priority: 1
assignee: cl-start@devbig030
issue_type: bug
labels:
- agentctl
created_at: 2026-11-02T18:07:43.775996897+00:00
updated_at: 2026-11-02T18:07:43.775996897+00:00
---

# Description

[opus 5.5] 2026-11-02 18:00:20Z on one host, a consumer team's bridge (build f0ec02a1, --offer-reply-command): the coordinator (a Claude session) printed a well-formed <CHAT_REPLY_N> block in a text-only message, but the bridge never captured it: the request stayed 'delivered', reply_count 0, nothing posted, no alarm. The agent record had no native session (native_session empty), so the transcript reader (#710) had nothing to read and screen capture missed it. Asks: (1) when an agent has no session binding, chat health says so and the bridge discovers and binds the session (as #741 does for revive: cwd plus last activity, confirmed by content); (2) the bridge alarms (coordinator and owner), not stays silent, when a delivered request gets no captured reply while the agent's transcript shows a CHAT_REPLY_<alias> block for it. Tests for both.
