---
title: 'replication-c9-lattice-fixes: hue eviction keeps tombstones; read marks become (epoch, max)'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- replication
created_at: 2026-10-23T14:41:08.716532114+00:00
updated_at: 2026-10-23T14:41:08.716532114+00:00
---

# Description

[opus 5.5] #454 replication-model C9 (F6, F8). Thread hues: a colour evicted at the per-channel bound is kept as a tombstone, so a thread read again is given back the colour it had (write-once survives eviction). read_marks: forget bumps a per-channel epoch instead of deleting all memory of the mark, and a mark may name the epoch it was based on, so a stale mark sent after a forget no longer re-advances it. Done stays last-action-wins (owner Q7, coordinator: LWW).

# Acceptance Criteria

Both backends (sqlite, fake) agree; migration ladder walked; tests for revival after eviction and for a stale mark after forget.
