---
title: 'wrkslots-remove-proof-mirror: one host-wide mirror per remote as the removal proof source'
status: closed
priority: 1
issue_type: feature
labels:
- wrkslots
created_at: 2026-11-16T02:30:00+00:00
updated_at: 2026-11-16T02:30:00+00:00
---

# Description

[watermelon-full-preview] Every slot removal proves commit
durability against the remote itself: one ls-remote per
repository and, when advertised tips are missing locally, one
batched commits-only fetch into an empty proof store. #1681
parts 1-2 made that once per repository per removal; under a
queue of dozens of slots the same remote is still asked the
same questions by every removal. The owner asked why cleanup
fetches per removal at all instead of keeping one local cache
of the remote up to date.

# Design

Design note: `ai_docs/plans/2026-11-15-wrkslots-remove-proof-mirror.md`.
One host-wide mirror per remote URL, coordinator-owned, one
entry per remote in its own collection directory. Removals
read proof evidence from the mirror when its last successful
fetch is within the documented 300 s freshness bound; the
fetch time is recorded in the salvage receipt. A stale,
missing, or unverifiable mirror is refreshed by one fetcher
under a per-mirror flock (incremental fetch), or the removal
falls back to today's direct proof. Removals never write the
mirror and never trust evidence older than the bound.

# Acceptance Criteria

- A removal whose mirror evidence is fresh does no network
  work of its own (measured by counting fetch/ls-remote calls
  in a queued-removal fixture).
- Stale or unverifiable mirror evidence is never trusted:
  refresh under flock, or direct-proof fallback, and the
  salvage receipt records which source and fetch time the
  proof used.
- Concurrent removals against one remote do not serialize on
  the mirror; only refresh does.
- The 300 s freshness bound is not widened; any failure of
  the mirror path fails closed to the direct proof.

# Notes

[watermelon-full-preview] Design approved by the coordinator
2026-11-15; implementation starts only on the coordinator's
explicit go. Safety analysis (force-push/delete inside the
freshness window, mirror ownership and disk, concurrent
writers) is in the design note.
