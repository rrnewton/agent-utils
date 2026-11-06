---
title: 'merge-queue-close-replayed-pr: a failing close silently blocks every later close, and close_pending never drains without a sync'
status: in_progress
priority: 2
issue_type: task
assignee: mu-mqdonor
created_at: 2026-11-06T20:01:38.109614209+00:00
updated_at: 2026-11-06T20:01:57.174616914+00:00
claimed_at: 2026-11-06T20:01:57.174616914+00:00
claimed_until: 2026-11-08T20:01:57.174516123+00:00
---

# Description

MEDIUM from coordb. Diagnosis (read-only): failing queue has every landed item close_pending=true, closed/ empty, no closed events, no limiter sync; close_postponed breaks on first Err and returns Ok, swallowing PR+error; retries only on landing wake or label sync. Fix: continue past failed closes with loud close-failed naming PR+error, drain close_pending from serve without --sync-label (budget-bound), fake-gh test.
