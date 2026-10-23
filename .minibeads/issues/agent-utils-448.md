---
title: 'send-dedupe: a slow multi-part send that looks failed is posted again on Retry'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- send
created_at: 2026-10-23T12:21:23.778605468+00:00
updated_at: 2026-10-23T17:54:41.661358568+00:00
closed_at: 2026-10-23T17:54:41.661358428+00:00
---

# Description

[opus 5.5] On a Slack deployment the owner sent a long message (about 5.6 KB, cut into three parts by the post cut). It reached the channel three times. The access log shows three POST /api/v1/channels/<channel>/reply requests logged at 12:13:12Z, 12:13:32Z and 12:13:55Z, each answered 200, taking 11914, 7504 and 7760 ms: every one of them posted all three parts. On 2026-10-22 around 20:20Z a three-part message arrived twice in the same way.

The owner confirmed the sequence: "that long message appeared to fail to send the first time, and then on retry appeared to succeed, but apparently that caused message duplication." So the page showed the first send as failed or unconfirmed while the server was still posting (or had posted) its parts, and Retry posted the whole message again. The Slack provider takes no idempotency key (`supports_idempotent_posts` is false), so the page sends none and the server has nothing to recognise a retry by.

Wanted:
1. Idempotent sends for every provider: the page attaches one key per composed message (its outbox id) on every attempt, the server remembers keys for a while and answers a repeat with the original result without posting again, coalescing a repeat that arrives while the first is still in flight. A deliberate new message with the same text gets a new key and posts.
2. Progress on the outbox row while a multi-part or slow send is in flight ("Posting part 2 of 3…"), and no way to send that message again until it settles.
3. Find and fix the page path that shows a send the server accepted as failed.
4. Check the Slack provider post timing (7-12 s for three parts) for waits between parts that can shrink without changing their order.

# Acceptance Criteria

Server: a repeated key posts once; a concurrent repeat is coalesced; a new key with the same text posts again. Page: a retry carries the same key; the row shows part progress; a regression test for the path that showed an accepted send as failed.

# Notes

[opus 5.5] Delivered in 132b896b (main): the server remembers every keyed post by its key for 24 h in memory, whatever the provider; a repeat with the same key, channel and text gets the original 200/207 and posts nothing even if its placement changed; a repeat while the first is posting is coalesced; the unsent rest of a 207 is the one other post a key may name; other reuse answers 409 key_reused. New route POST .../reply/outcome (200/207, 202 with part progress, 404 post_unknown). The page sends its key to every provider, freezes reply_to/thread_id per key across retries and reloads, asks the outcome route after a lost answer (page timeout, dropped connection, gateway 5xx in HTML or JSON) before calling a send unconfirmed, and shows "Posting part N of M…" while a send is slow. Slack: the later parts of a long post skip the conversations.replies read of the thread their first part just started. Cause of the incident as far as the code shows: the page never got the first POST's answer (something in front of the server answered first, or the connection dropped) and the unkeyed Retry posted again; the deployment's front-door timeout was not checked (live config, out of bounds for this lane). Unverified: persistence across restart (not done, see the follow-up), a lost answer for the first part of a post on a provider without upstream keys (can still post that part twice).
