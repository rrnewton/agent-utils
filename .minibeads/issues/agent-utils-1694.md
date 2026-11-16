---
title: 'bridge-tail-single-ingest: one event tail per channel instead of a tail per consumer'
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

Every Slack consumer on a host runs its own event tail as the owner: each chat bridge opens `slack.conversation tail` for its channels, and vibe-talk separately tails the same channels for live ingestion. The tails duplicate ingestion, multiply owner-identity subscriptions, and each consumer re-derives the same message stream. The bridge tail must run as a channel member, so it cannot simply be replaced by an app-identity subscription.

# Design

Keep exactly one tail per channel per host. The bridge consumes vibe-talk's live feed of stream records (which vibe-talk already normalises as a read would), or vibe-talk consumes the bridge's stream records where the bridge is the tail owner; the other consumer subscribes to that feed instead of opening its own tail. Catch-up, sequence cursors and gap behaviour stay with the single tail owner, and a consumer resuming from its cursor loses no message.

# Acceptance Criteria

- With both a bridge and vibe-talk running over the same channel, exactly one `slack.conversation tail` process exists for that channel.
- A message posted to the channel reaches both consumers once, in order, including after a restart of either consumer.
- A reported gap in the single tail surfaces to both consumers instead of being hidden by the other's stream.

# Notes

[watermelon-full-preview] Filed by the direct-Slack burn-down enumeration (owner request 105, 2026-11-15): the target model is client <> vibe-talk <> Slack, and this is one remaining path that reaches Slack without vibe-talk's cache or writer in between. Rates and the shared-budget accounting are in #1682 slack-rate-limit-budget.
