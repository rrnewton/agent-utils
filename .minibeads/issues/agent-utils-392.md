---
title: 'chat-reply-no-wake: chat reply stores a reply but a running chat run without --offer-reply-command never hears of it'
status: closed
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-10-20T07:47:56.371486277+00:00
updated_at: 2026-10-20T07:53:17.951809453+00:00
closed_at: 2026-10-20T07:53:17.951809353+00:00
---

# Description

[opus 5.5] On 2026-10-20 agentctl chat reply --reply-id 001 stored a reply and printed service_woken: false; the running Rust chat run service did not send it until it was restarted. The Rust service bound its .wake.sock only with --offer-reply-command, while the Python edition always listens. chat reply is documented for replies printed between marker lines that were not captured, which happens without the option too.

# Acceptance Criteria

chat run listens on .wake.sock whenever outbound is enabled, whatever --offer-reply-command says, so chat reply wakes it and the reply is sent at once; a test covers it; help and guide say so.
