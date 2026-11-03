---
title: 'chat-no-stale-replies-on-restart: a restarted bridge must not post long-queued replies unasked'
status: closed
priority: 1
issue_type: bug
assignee: cl-start@devbig030
labels:
- agentctl
created_at: 2026-11-02T17:39:26.214515020+00:00
updated_at: 2026-11-03T06:13:29.845002641+00:00
closed_at: 2026-11-03T06:13:29.845002531+00:00
---

# Description

[opus 5.5] A consumer team's restore (2026-11-02, their gap 28): after a bridge refresh to a new build, the bridge posted to the owner a reply that had sat in its outbound queue for 4 hours. A restarted bridge must not post queued replies older than a bound (configurable; default e.g. 15 min), or replies to requests already answered by a later reply, without the coordinator's say-so: list them (chat status / health and a prompt to the coordinator) and let it send, edit or drop each one. Fresh replies still go out immediately. Tests: a stale queued reply survives a restart and is held and listed, not posted; a fresh one is posted.

# Notes

## Resolution

[opus 5.5] When `chat run` starts, it holds back each unsent reply in either of two cases: it was captured `--hold-replies-older-than` ago or longer (default 15m; `0` holds only the second case, `off` holds nothing), or its thread has moved on, meaning a later request in the same thread already has a reply sent. This is how "answered by a later reply" is read. The held list lives in `held-replies.json` in the state directory. The publisher skips a held reply, and that request's later replies wait behind it, in order. Replies captured after the start are never held.

The coordinator is told once, with a typed notice like a reply reminder, giving each reply's start and the exact `chat held --send` / `--drop` commands. Each held reply is also logged as `HELD STALE REPLY`.

`chat held` lists the held replies, as text or as `--json` (schema `agentctl-chat-held/v1`). `--send KEY/ORDINAL` takes the hold off. `--drop KEY/ORDINAL` marks the reply never to be sent. To change one, drop it and send the new text with `chat reply`. Both wake a running service.

`chat status` has a `held_replies` field. `chat health` reports held replies as DEGRADED, lists each one, and does not also count them as stuck unsent.

Tests:
- `a_stale_reply_is_held_at_start_until_the_coordinator_sends_or_drops_it`: a fresh reply is not held, and a stale one is held, listed, not sent until released, and never sent once dropped.
- `a_reply_whose_thread_has_moved_on_is_held_even_when_fresh`.
- `the_held_replies_notice_names_the_commands_that_send_or_drop_each`.
- `replies_held_after_a_restart_are_degraded_and_not_called_stuck`.
- The CLI test `chat_held_lists_a_held_reply_and_send_takes_the_hold_off`.
