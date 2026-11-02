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

# Notes

[opus 5.5] Same host and bridge build, 18:00:36Z: the bridge HUNG while submitting a request into a busy Claude pane. Its main loop stopped (no log after 17:54Z, no replies sent) while the process stayed alive, and chat health said only DEGRADED. A restart at 18:12Z fixed it. Asks: (3) every blocking step of the main loop has a bound (time out and continue), a stalled loop is detected by a watchdog heartbeat and reported as a distinct health state (not DEGRADED), and the service restarts itself on a stalled loop. Evidence to follow from the other host.

[opus 5.5] Also (owner saw it): the same request got TWO posts 2 s apart after the bridge restart. Reply 1 came from `chat reply N --file` (text taken from the transcript; Markdown table correct); reply 2 was "captured from the coordinator's pane" and its table was mangled by screen rendering. Asks: (4) dedupe: a reply ID already answered with `chat reply` (or any reply with the same text) is never also posted from a capture; (5) pane capture is the last resort: never use it when the transcript is known (the same pass took the next request from the transcript), and prefer refusing with an alarm over posting mangled tables.
