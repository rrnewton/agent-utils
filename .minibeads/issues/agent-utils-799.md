---
title: 'chat-reply-id-leading-dash: a reply ID that starts with ''-'' cannot be passed to agentctl chat reply'
status: open
priority: 3
assignee: cl-start@devbig030
issue_type: bug
labels:
- agentctl
created_at: 2026-11-02T23:58:31.571396636+00:00
updated_at: 2026-11-02T23:58:31.571396636+00:00
---

# Description

[opus 5.5] Found by cl-start 2026-11-02 as a flake in rust.agentctl.test (chat_reply.rs): when a chat request has no short alias, its reply ID is <nonce>_<ordinal>, and the random nonce can start with '-'. Then 'agentctl chat reply <ID> --file F' (the exact command every bridge prompt prints) fails because clap reads the ID as an option. Fix: never generate a nonce that starts with '-' (and accept '--' before the ID as well), and make the printed command robust (e.g. 'chat reply -- <ID>' when needed). Test: a forced leading-dash nonce round-trips through the printed reply command.

# Notes

[opus 5.5] Correction from cl-start: the reply command bridges print is already safe (short_reply_command uses --reply-id ID, and positional_word puts -- before an ID starting with -). Failures come only from passing such an ID positionally (the test, or an agent typing it by hand). Fix: new reply nonces never start with - (redraw), plus a regression test; existing open requests rely on the safe printed forms. Lowered to P3.
