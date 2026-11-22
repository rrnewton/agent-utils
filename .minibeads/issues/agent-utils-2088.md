---
title: 'wrkslots-recover-absent-check-before-locks: recover-absent-validate-rows holds every lock for minutes just to refuse'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-22T03:50:00-05:00
updated_at: 2026-11-22T03:50:00-05:00
---

# Description

`wrkslots recover-absent-validate-rows --apply` takes all four registry locks, including the state-root flock, then loads the whole event history (about 95,000 files on one consumer host) and only then runs a Git-registration check that refuses ("Git still registers absent validation checkout"). Each call held the locks for 2-3 minutes just to refuse; called once per row by a consumer's reaper, it blocked heartbeats, validation launches and disk snapshots for over an hour.

# Design

Run the cheap refusal checks (Git registration, and any other precondition that does not need the registry) BEFORE taking the locks, the way the process census already runs outside them, then re-validate under the lock before acting. Load only the event history the decision needs (the event index where possible).

# Acceptance Criteria

- A refusable row refuses without taking the state-root flock; an accepted row still re-validates under the lock; the lock hold time no longer scales with the full event history; deterministic tests.

# Notes

Filed by the coordinator from a consumer report; the owning lane refines this file as it works.
