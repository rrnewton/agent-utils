---
title: 'send-dedupe: a slow multi-part send that looks failed is posted again on Retry'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- send
created_at: 2026-10-23T12:21:23.778605468+00:00
updated_at: 2026-10-23T12:21:23.778605468+00:00
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
