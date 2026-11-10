---
title: 'chat-duplicate-refusal: agentctl chat refuses a near-duplicate post in the same channel and thread'
status: closed
priority: 1
issue_type: feature
labels:
- agentctl
- chat
assignee: mu-chatdup
created_at: 2026-11-10T17:54:21+00:00
updated_at: 2026-11-10T18:33:00+00:00
closed_at: 2026-11-10T18:33:00+00:00
---

# Description

[watermelon-full-preview] Owner request (owner reply 339, 2026-11-10 12:49 PM ET): "Maybe we could have a chat bridge refusal with "this is a 99% match to your previous message which went through at HH:MM, are you sure you want to send? If so try again with --allow-duplicates". We could try to efficiently check against recent messages..." Context: an agent published the same announcement twice by retrying with a new request id after misreading output, and the owner says this "has happened pretty often".

Today `agentctl chat reply` dedupes only the identical text within one request (`chat_service::reply` stores once, and the service sends each stored reply once), and `agentctl chat publish` dedupes only at the outbound helper by its caller-owned request UUID (`chat_service::publish` writes no bridge state and creates no reply route). A retry with a new request id and the same or nearly the same text posts again, in either command.

# Design

- On `chat reply` and `chat publish`, compare the text against the same sender's recent outbound posts in that channel: the last 30 minutes of bridge-recorded outbound posts (replies the bridge holds, plus publishes recorded in the bridge state), read from the bridge's own records and ledger only; no provider API calls.
- Scope is one channel and one thread: a reply answers its request's channel and thread, a publish names its channel and optional thread; a post to a different thread is not a duplicate.
- Use a normalized text hash for exact matches plus a cheap similarity check (word shingles with Jaccard at or above 0.99) for near matches, so one word changed in a long text still matches.
- On a match, refuse with a nonzero exit code and a message naming the earlier post's time (in ET) and its id, plus `--allow-duplicates` to override.
- A retry with the same request id stays an idempotent no-op success (publish's helper dedupe, reply's already-stored outcome), never a refusal.
- Rust is the primary edition; keep the Python mirror in parity for the same commands, per the repository's edition rules.
- The services run snapshots; nothing changes live until the next turnover.

# Acceptance Criteria

- An exact repeat in the same channel and thread within the window is refused, naming the earlier post's ET time and id and the `--allow-duplicates` override.
- A near match (one word changed in a long text) is refused the same way.
- A post to a different thread, and a post outside the 30-minute window, go through.
- A retry with the same request id succeeds as an idempotent no-op and never refuses.
- `--allow-duplicates` sends a matching post.
- Focused tests cover each case above for reply and for publish in the Rust edition, with the Python mirror kept in parity.

# Notes

Tracker search 2026-11-10: nothing open covers this. #448 send-dedupe is the vibe-talk page Retry path, a different path (closed). Related: the reply store's per-request text dedupe and the publish helper's per-UUID dedupe, which this refusal sits in front of.

Resolution (2026-11-10): delivered in this lane's commit. Rust: BridgeState::duplicate_match scans recorded publishes (publishes.json) plus held replies in the same channel and thread over the last 30 minutes (normalized-token exact hash, then word-bag Jaccard at 0.99 or more); chat_service refuses before the helper or the store with the earlier post's ET time and id, --allow-duplicates overrides, and a same-request-id publish retry answers from the ledger without posting. Python reply carries the same refusal over its own records. Focused tests cover exact, near (one word in 220), different thread, same request id, override, and outside the window. The services run snapshots; nothing changes live until the next turnover.
