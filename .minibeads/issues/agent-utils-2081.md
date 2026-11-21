---
title: 'wrkslots-census-scope-under-churn: the remove in-use census must close on a busy host and not hold the state lock'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T18:35:00-05:00
updated_at: 2026-11-21T18:35:00-05:00
---

# Description

On a busy host, wrkslots remove refuses because its in-use census never closes: it inventories every process on the host, and unrelated processes keep forking and exiting. The read-only census also runs while holding the state-root lock, so a slow census blocks every other wrkslots command.

# Design

Judge only processes that could touch the slot (for example by the slot's filesystem or device identity, or by open files and cwd under the slot's paths) instead of every process, so the census closes under churn without weakening the refusal for a real holder; and run the read-only census outside the state-root flock, re-validating whatever the decision depends on under the lock before acting.

# Acceptance Criteria

- Remove of an unheld slot succeeds with unrelated process churn on the host; a real holder still refuses.
- The state-root lock is not held during the census.
- Deterministic tests on a controlled process inventory.

# Notes

Filed by the coordinator from a consumer report; the owning lane refines this file as it works.
