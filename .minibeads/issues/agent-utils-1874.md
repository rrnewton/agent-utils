---
title: 'wrkslots-create-remove-long-config-holds: creates and removes hold the exclusive lock across slow phases'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
- lock-scope
created_at: 2026-11-18T18:45:00+00:00
updated_at: 2026-11-18T18:45:00+00:00
---

# Description

[watermelon-full-preview] Consumer measurement, 2026-11-18: slot creates took up to ~15 minutes each under lock contention, an Eden create took 313 s, and removes held the configuration/state locks 21-44 s each. #1808 bounded the waits (a heartbeat now queues at most min(10 s, TTL/3) by default) and shortened heartbeat holds to ~30 ms, but a minutes-long exclusive hold still starves every waiter whose bound is shorter. The long holds are the bug; bounded waits only make them visible.

# Design

Creates (including Eden creates) must not hold the exclusive configuration lock across planning, provisioning, submodule work or post-provision hooks: prepare lock-free (as create already does for validation), take the locks only to commit the record and journal, and provision outside them under the slot fence, as removal already deletes outside them (#894). Removes on the non-prepared path similarly run census and salvage derivation before the lock where the prepared path does not already. This continues #1361 create-provision-unlocked; each command converts separately with counted (never timed) tests proving no slow phase runs under the lock.

# Acceptance Criteria

- No create, Eden create or remove phase that provisions, censuses or salvages runs while holding the exclusive configuration lock, proven by operation counts under the lock.
- Heartbeats are never starved by a create's slow phase once stage E (lock-free heartbeat coherence) also lands.

# Notes

Filed from the #1808 stage-E analysis; independent of the config.lock split (stage D).
