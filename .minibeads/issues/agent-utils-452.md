---
title: 'slack-live-ingest: ingest the Slack tail''s message text directly instead of re-reading it'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- slack
- live
- cache
created_at: 2026-10-23T13:16:00.422275527+00:00
updated_at: 2026-10-23T14:12:53.127886333+00:00
closed_at: 2026-10-23T14:12:53.127886164+00:00
---

# Description

[opus 5.5] The Slack live hint throws away the message text the provider tail already carries and then re-reads the channel through the bridge, adding a provider round trip (and a 5 s hint floor under load) to every arrival. Owner: 'If we already have the messages, we should just immediately ingest them into our cache and serve them to the client.' Build: an authenticated content-carrying ingest path for tail records (strict validation, same normalization as a read so a later read merges as the same message), one tail for every configured Slack channel with a persisted sequence cursor and GAP handling, a periodic cheap relist of each channel's newest page and recently active threads to pick up edits, deletions and other bots' posts, and latency measurements before and after on the fake Slack path. Contentless hints stay as a fallback mode.

# Acceptance Criteria

Pushed message.created records reach the live stream and the message cache without a provider read; a later read of the same message merges as the same message and never republishes it as new; GAP forces a re-read; the relist publishes edits and deletions; adapter self-test and server tests cover each part; send-to-visible latency measured before/after on the fake.

# Notes

[opus 5.5] Delivered: POST and GET /api/v1/live/tail (content-carrying, strictly validated, the same normalization as a read), created-memory deduplication with the poller, history rules, GAP and reaction handling, the relist ([ingest] relist_seconds), live-hint-adapter.py --records (every conversation, persisted cursor, overflow and unreadable lines reported as gaps), tests, and fake-Slack latency figures (README, "Stream records"). Unverified: the real tail's reaction record payload is not interpreted (a reaction only wakes the poller), and nothing has run against the live stream; deployment and switching the adapter to --records are the coordinator's.
