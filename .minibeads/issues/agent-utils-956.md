---
title: 'wrkslots-remove-stale-lsof-after-lock-wait: remove refuses when the lock wait ages the lsof scan past its freshness window'
status: open
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-06T19:14:11.000000000+00:00
updated_at: 2026-11-06T19:14:11.000000000+00:00
---

# Description

Remove of a dead consumer slot refused with "the lsof scan ... is more than 300 seconds old at its first use check under the registry lock". The scan was taken before waiting for the registry lock, and the wait used up its 300 s freshness window.

# Design

- When the scan has gone stale by the time the lock is held, rescan under the lock, or do the scan after taking the lock, instead of refusing. Keep the freshness check itself.
- Bound the rescan so a slow lsof under the lock cannot hold the registry for long, for example by limiting it to the slot's paths.

# Acceptance Criteria

- A test with a lock wait longer than the freshness window: remove rescans and proceeds, and still refuses if the rescan finds the slot in use.

# Notes

Reported 2026-11-06 by the lead coordinator from a consumer registry. Part 3 of 3 with #954 and #955.
