---
title: 'vibe-talk-thread-naming-from-cache: name threads from the local cache, not Slack'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-18T21:55:00-05:00
updated_at: 2026-11-18T21:55:00-05:00
---

# Description

Owner report (thread 211, 2026-11-18): the 014 team gives no thread slug names at all, neither on thread cards nor on the Thread button on main-channel messages. Live app.log shows summarize::threads WARN "a thread-naming read failed at the chat provider" (Slack rate limit on conversations.replies, channel pause 600s) about every 10 minutes all day; only a few dozen namings succeeded. Since #1847 background reads share a learned Slack ceiling at its 4/min floor, naming starves. Thread slugs do not come from the agent bridge: agentctl publishes only short-refs.json (number to channel/root), and vibe-talk joins its own filed name onto it; no bridge refresh can supply a slug.

# Design

- Naming already tries the cache first (#1563), but SinkThreadCache gates on the page-serve proof (cache::held::whole: exact reply count, latest reply, coverage claims listed within 15 minutes), so threads held from older listings or pushes miss and fall back to conversations.replies, which rate-limits and pauses the channel.
- Relax the naming read to a held-rows cache read: when the thread scope holds the root and replies, name from those rows with zero Slack calls, without the freshness/coverage/count proof a served page needs. Keep the proven-whole read first where it applies; fall back to Slack only when the cache holds too little of the thread, at background (naming) priority as today.
- Backfill follows the same path: unnamed threads already queued from page reads are named from the cache after deploy, spending no Slack burst; no new upstream scan is added.
- Report the top Slack budget consumers from the slack_calls report lines in app.log (by method and consumer) in the handoff.

# Acceptance Criteria

- On a test instance with a seeded cache, naming a held thread makes zero chat-provider calls (fake call count asserted) and the name shows on thread cards and on the main-channel Thread button.
- A thread the cache does not hold still falls back to one upstream read at naming priority; a cache naming spends neither the per-channel interval nor a channel pause.
- A cheap unit/integration test covers cache-hit (zero provider calls) and cache-miss fallback; dynamic end-to-end exercise through the real server router on a test instance (never the live instance) with screenshots per the standing matrix, BUILD.txt included.

# Notes

Owner-priority, deployed on landing. Coordb checks the live channel read-only after deploy.
