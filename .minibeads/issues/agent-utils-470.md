---
title: 'replication-c8-root-count: the cache counts a new reply on its held root'
status: closed
priority: 2
issue_type: task
labels:
- vibe-talk
- replication
created_at: 2026-10-23T17:02:42.013074821+00:00
updated_at: 2026-10-24T00:55:29.202593644+00:00
closed_at: 2026-10-24T00:55:29.202592963+00:00
---

# Description

[opus 5.5] #454 replication-model C8. A reply the cache did not hold proves its root has at least one reply; where the held root still says none, raise it to one (not exact), so a root served from the cache is not drawn unanswered beside an own reply. The provider count replaces it with the next copy of the root.

# Acceptance Criteria

Writer test: own reply raises 0 to 1 inexact; a newer root copy replaces it.
