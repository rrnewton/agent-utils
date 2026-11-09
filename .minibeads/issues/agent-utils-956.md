---
title: 'wrkslots-remove-stale-lsof-after-lock-wait: remove refuses when the lock wait ages the lsof scan past its freshness window'
status: closed
priority: 2
issue_type: bug
assignee: mu-wrkslots
created_at: 2026-11-06T19:14:11.000000000+00:00
updated_at: 2026-11-09T05:50:00.000000000+00:00
closed_at: 2026-11-09T05:50:00.000000000+00:00
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

## Resolution

Resolved by #1108 remove-lock-hold-starves-validate (be8b3a8e), plus removal of the refusal it made pointless (cl-wrkid, 2026-11-08 ET). The lead's question was whether any deletion decision still rests on pre-lock evidence alone. It does not:

- An image-backed slot's decisive check is its `before_image_fence` lsof run under the locks, which is fresh by construction. A single remove of an image-backed slot runs the ordinary locked checks and has no shared scan to age.
- For every other agent slot, the decisive check is the scan after the path fence. It compares every process with the fenced tree by path and by device and inode, so it also covers the inode-alias use the shared lsof scan was kept for. It re-lists /proc (#1102), and a use rolls the fence back.

So the batch-of-one refusal "the lsof scan ... is more than 300 seconds old at its first use check under the registry lock" protected nothing, and it was the refusal reported here. It is removed. A long lock wait now only means the early refusal is older. No scan runs under the lock, and no retry loop was needed. remove-agent-batch still takes a new shared scan, outside the lock, before an item whose scan is older than `_AGENT_REMOVE_BATCH_CENSUS_MAX_AGE_SECONDS`. The Rust edition's single remove has no shared scan, so there is nothing to mirror.

Tests (py/wrkslots/tests/test_lifecycle.py):

- `test_remove_goes_on_after_a_lock_wait_longer_than_the_scan_bound`: every scan counts as stale and the remove proceeds. It replaces the test of the old refusal.
- `test_remove_finds_a_use_that_began_during_its_lock_wait`: a process enters the slot after the unlocked scan, while remove waits for the lock with stale evidence. The scan after the fence names it at the fenced path, the fence is rolled back, and the slot and its record are intact.

