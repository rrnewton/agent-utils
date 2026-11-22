---
title: 'wrkslots-history-read-outside-config-lock: read the event history before taking config.lock, then recheck under it'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-22T13:15:00-05:00
updated_at: 2026-11-22T13:15:00-05:00
---

# Description

Commands that replay the full event history (about 95,000 files on one consumer host) do so while holding worktrees/config.lock: recover-absent-validate-rows holds it about 2.3 minutes per call for rows that pass its pre-checks, and slot moves hold it 5 to 6 minutes. Back-to-back calls starve slot heartbeats waiting 300 seconds, which then get refused. #2088 removed the lock hold only for rows that refuse early.

# Design

Prove-then-recheck: read and replay the history outside the lock, take the locks, verify the snapshot revision and event-log tip are unchanged since the read (cheap), and only then mutate; if anything moved, redo the read (bounded retries) or fall back to the locked path. Apply it to recover-absent-validate-rows and to slot moves first, then to other long locked replays. Measure lock hold before and after.

# Acceptance Criteria

- Lock hold for a passing recover-absent call and a slot move drops to seconds on a large history fixture; a concurrent mutation between read and lock is detected and handled; tests for both.

# Notes

Follow-up to #2088; event-store compaction stays in #1123. Filed by the coordinator.
