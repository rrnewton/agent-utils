---
title: 'queue-supersede-key: a newer queued message with the same key replaces the older undelivered one'
status: open
priority: 1
issue_type: feature
assignee: mu-inbox
labels:
- agentctl
created_at: 2026-11-09T17:05:46.099746999+00:00
updated_at: 2026-11-09T17:14:33.279258850+00:00
---

# Description

Owner request 2026-11-09 (replies 248-251), via the lead coordinator. When a coordinator lane blocked on an in-TUI question on 2026-11-09, every queued prompt backed up behind it; on unblock a flood arrived, including repeated identical #862 watch-status-requests re-pages and stale periodic notices. The owner: messages should never back up (attack at source), and if they do, automated messages should carry a TTL and/or a deduplication key so a second replaces the first.

agentctl send already has --ttl SECONDS (expired prompts are set aside untyped) and --message-id (a repeat of the SAME id is a no-op). There is no supersede key: a newer notice about the same subject cannot replace an older still-undelivered one, so both are delivered.

Add a supersede key to the send queue in both editions where the queue exists (Rust primary; Python edition to match, wire-compatible): when a message is enqueued with a supersede key, any older UNDELIVERED queued message for the same target with the same key is replaced (never typed); delivered history is kept and the replaced entry is retained in history marked superseded (by which message, when). Ordering is preserved: the replacement keeps queue semantics documented in the userguide (state explicitly whether it takes the old position or appends last, and test it). Inflight/possibly-submitted entries are never replaced. Queue files written by older builds (no supersede field) must still load. Caller migration (every periodic notice setting TTL + key) is the sibling issue automated-notice-ttl.

# Acceptance Criteria

- agentctl send accepts a supersede key in both editions where the queue exists; userguide and quickstart document it beside --ttl and --message-id, including key scope (per target) and ordering.
- Tests: a newer queued message with the same key replaces the older undelivered one (older never typed, kept in history marked superseded); an expired message is still set aside untyped (TTL composes with supersede); ordering relative to other queued messages is as documented; an inflight message is not replaced; old queue entries without the field load unchanged; cross-edition differential agrees where one exists.
- Full gate passes (rs/agentctl touched: no exclusive waiver).
