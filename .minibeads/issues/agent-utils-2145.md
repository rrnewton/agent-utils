---
title: 'wrkslots-create-journal-blocks-unrelated-heartbeats: one slot interrupted create refuses every other owner heartbeat'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-23T00:45:00-05:00
updated_at: 2026-11-23T00:45:00-05:00
---

# Description

While one slot's create journal was left behind by a failed post-provision hook, every lane's heartbeat on the same registry was refused with "interrupted mutation for slot '<slot>' ... run wrkslots recover first". One lane's lease TTL lapsed on the registry clock as a result. A failed create of one slot should not block per-slot operations of unrelated slots and owners.

# Design

- Scope the interrupted-mutation refusal: per-slot operations (heartbeat, hold, handoff, release) on a different slot proceed when the outstanding journal names only another slot and touches nothing they read or write; registry-wide operations still refuse.
- Keep the check constant-time under the lock (journal names come from the journal directory listing done outside the lock and are validated with a generation check).
- Report the outstanding journal as a warning on the unrelated operation, with the recover command.

# Acceptance Criteria

- Test: with a CREATE journal outstanding for slot A, a heartbeat for slot B succeeds (with a warning) and a heartbeat for slot A refuses.
- No change for operations that touch slot A or the registry as a whole.

# Notes

Reported by a consuming team on a shared host; filed by the coordinator. Input to the per-slot locking work (#2127) and the wrkslots V2 design (#2138).
