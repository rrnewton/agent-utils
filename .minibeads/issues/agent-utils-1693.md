---
title: 'bridge-rescan-relay-cache-reads: answer bridge history and thread listings from the vibe-talk cache'
status: open
priority: 2
issue_type: task
labels:
- slack
- vibe-talk
created_at: 2026-11-16T02:00:00.000000000+00:00
updated_at: 2026-11-16T02:00:00.000000000+00:00
---

# Description

The Slack chat bridge is a second Slack gateway. Its relay poll and its general rescan list channel history and threads directly through a Slack command-line client (`slack.message list`, i.e. `conversations.history` and `conversations.replies`) on independent schedules (relay poll configured at 15 seconds, floored at 30 seconds in code; rescan every 90 seconds over a day's window), duplicating channels the vibe-talk server also reads and holds in its message cache. In the #1682 measurement window the two bridges on one host sent about 29% of all `conversations.history` calls. They also keep a second, separate per-minute listing budget from vibe-talk's own, so two accountants spend one shared upstream bucket.

# Design

Add a cache-backed listing transport to the Slack subscription plugin: history and thread listings are answered from the vibe-talk server's message cache using the same proven-whole proof a page read uses, with an upstream listing only on a cache miss, and one shared rate budget with vibe-talk. The rescan then compares cache state instead of re-listing history; the relay poll reads the cache at its cadence. Keep the stream/rescan dedup ledger semantics unchanged: a message is admitted once, and a rolled-back admission is offered again identically.

# Acceptance Criteria

- A relay poll and a rescan pass over a channel whose history the cache holds proven-whole make zero upstream `conversations.history` or `conversations.replies` calls.
- A cache miss falls back to one upstream listing, which then fills the cache for the next pass.
- Bridge listings and vibe-talk reads share one per-minute budget; exhausting it pauses both, with the cause logged.
- Existing rescan, relay and dedup tests pass, plus a new test proving the zero-call cache-hit pass.

# Notes

[watermelon-full-preview] Filed by the direct-Slack burn-down enumeration (owner request 105, 2026-11-15): the target model is client <> vibe-talk <> Slack, and this is one remaining path that reaches Slack without vibe-talk's cache or writer in between. Rates and the shared-budget accounting are in #1682 slack-rate-limit-budget.
