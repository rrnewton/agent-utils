---
title: 'vt-live-push: apply chat event streams directly, and never show a partial thread as complete'
status: open
priority: 0
issue_type: bug
created_at: 2026-10-30T16:38:09.735068150+00:00
updated_at: 2026-10-30T16:38:09.735068150+00:00
---

# Description

[opus 5.5] Slug: vt-live-push (also covers vt-reaction-events).

## Problem

The owner sees new messages, thread replies and reactions in the chat client at once, but vibe-talk shows them late, or shows part of a thread, even after a reload.

Measured on the live Slack instance (2026-10-30):

- The records-mode adapter (#452 slack-live-ingest) tails the conversations as the owner. That stream carries only the owner's own posts: about 4 h of events, 36 `message.created`, every one by the owner, none from the chat bridge's app, and no reaction records at all ("Posts by other bots, message edits and deletes are not emitted"). So the bridge's replies and its reactions reach vibe-talk only through polling reads, and under the shared per-person rate limit 4 to 28 of those reads a minute are refused.
- **Partial thread (ingest bug).** The poller keeps where it stands in each thread only in memory. After a restart, its first read seeds every thread as read to the root's `latest_reply` without reading the replies. Replies that earlier refused reads never fetched are then skipped for good: the next tick reads only what came after the seed. A reply posted after the seed was published alone while the three before it stayed missing until a page read happened to fetch the thread about four minutes later.
- The page's thread reads were answered from the server's copy (their own reads refused), and nothing in the answer said the thread was incomplete, although the root's reply count was higher than the replies held.

## Wanted

1. Never present a thread or list as complete when the server knows, or cannot rule out, that messages are missing: a root counting more replies than are held, stream records seen but not applied, a gap in the stream, a refused read.
2. After a restart, the poller reads the replies of any thread whose root counts more replies than the cache holds, instead of assuming them read.
3. A tail layer that merges N event sources by event id: the owner tail plus one tail as the chat bridge's own app identity today, more identities later. Message and reaction events are applied directly to the cache and published at once. Reads become reconciliation. Senders that no tail covers still arrive through those reads.
4. Each fact (new message, reply, reaction, edit, delete, thread-root status) is published on its own, never held back until another fact triggers a re-read.
5. Root-cause the adapter's memory (about 900 MB against a 1 GB MemoryHigh) and the upstream tail's frequent exits.
