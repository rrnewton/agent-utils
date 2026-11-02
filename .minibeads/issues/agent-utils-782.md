---
title: 'agent-teams-relay-mirror: mirror every inter-team message to the relay channel while delivering directly'
status: open
priority: 2
assignee: mu-mirror@devbig014
issue_type: feature
labels:
- agent-teams
depends_on:
  agent-utils-770: blocks
created_at: 2026-11-02T17:42:56.458057350+00:00
updated_at: 2026-11-02T17:42:56.458057350+00:00
---

# Description

[opus 5.5] Owner approved 2026-11-02. A route option (e.g. "mirror": true on relay-mode routes in routes.d) under which every inter-team message is still delivered over the direct route, AND the same signed envelope (same ID) is also posted over the mirror route, so the owner can watch all traffic in the relay channel. The receiver's ledger deduplicates, so the copy is never delivered twice. Mirror only teams that declare relay_channels. Never mirror --private messages. A mirror failure never changes the delivery state. Pairs with #770 agent-teams-relay-fallback-route (same lane, on the other host).
