---
title: 'automated-notice-ttl: every periodic automated notice sets a TTL about its period and a supersede key'
status: open
priority: 1
issue_type: feature
assignee: mu-inbox
labels:
- agentctl
depends_on:
  agent-utils-1156: blocks
created_at: 2026-11-09T17:05:54.251680112+00:00
updated_at: 2026-11-09T17:14:33.411573092+00:00
---

# Description

Owner request 2026-11-09 (replies 248-251), via the lead coordinator; depends on the queue supersede key (queue-supersede-key). The 2026-11-09 unblock flood was mostly our own automation: #862 watch-status-requests re-pages every 30 minutes until ANSWERED (identical apart from age minutes), watch queue-stale notices, watch-agents idle/stall notices, and tick-hub ticks/scans. None of them set --ttl, so stale copies sat in queue and were all eventually delivered.

Make every periodic or automated notice self-expiring and self-replacing: TTL about equal to its period, plus a supersede key scoped one per request, lane, or check. In scope: (1) watch-agents #862 status-request re-pages (key per agent + heading/request; TTL about repage_minutes); (2) watch queue-stale notices; (3) watch-agents idle and stall notices (key per lane + notice kind); (4) tick-hub dispatch to agentctl send (key per reminder/check name; TTL about its cadence where a cadence exists). Human and lane-authored messages are untouched. Where a notice today bypasses the queue and types directly, say so in the resolution rather than silently rerouting it.

# Acceptance Criteria

- Each in-scope automated sender sets TTL approximately its period and a supersede key (one per request, lane, or check), in both editions where that sender exists; config overrides documented where the period is configurable (for example repage_minutes drives the #862 TTL).
- Tests per sender: two fires while the target is unresponsive leave at most one undelivered notice; a notice older than its TTL is never typed; first-fire behaviour (what the coordinator sees when all is well) is unchanged.
- Userguide documents the keys/TTLs per notice kind. Full gate passes.
