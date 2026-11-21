---
title: 'agentctl-archive-strands-queue: archived coordinator queues strand undelivered items'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- agent-teams
created_at: 2026-11-21T02:50:00-05:00
updated_at: 2026-11-21T02:50:00-05:00
---

# Description

[watermelon-full-preview] Items queued for a coordinator record that was later archived or replaced ended in the archived record's queue with delivery_state queued or inflight, were never typed, and nothing raised them. Failed items were also left inflight. v2 scope (cold review): no re-targeting.

# Acceptance Criteria

- On any archive, replace, retire or unregister path, undelivered items are set aside under the queue delivery lock, respecting live claims: pending cancelled stranded-archived (stranded-expired past not_after), maybe-typed inflight quarantined possibly_submitted. Never processed, never re-enqueued. One coalesced notice to the current designated coordinator lists sender, id, first line and first queued time in ET; a human re-sends.
