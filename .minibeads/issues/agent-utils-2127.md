---
title: 'wrkslots-finer-grained-locks: per-slot locks for per-slot operations, and no lock on a consumer state root'
status: open
priority: 2
issue_type: feature
labels:
- wrkslots
created_at: 2026-11-22T14:30:00-05:00
updated_at: 2026-11-22T14:30:00-05:00
---

# Description

The owner asked whether the wrkslots locking can be finer-grained. Today a heartbeat, hold or handoff of ONE slot takes the host-wide exclusive config.lock, so long cross-slot operations starve unrelated per-slot work. Some commands (recover-absent-validate-rows) also hold an exclusive flock on the consumer's state root, which that consumer uses as its own run-population lock.

# Design

Split locking: per-slot operations (heartbeat, hold, handoff, release of one slot) take that slot's own lock plus, at most, a short shared lock on the registry; only cross-slot or registry-shape changes take the exclusive config.lock. wrkslots never locks a consumer's state root; it locks only files under its own control directory. Define the lock order to stay deadlock-free, and model it in the TLA+ spec (#2090). Builds on #2125 (reading history outside the lock) and #1123 (event-store compaction).

# Acceptance Criteria

- A per-slot heartbeat proceeds while a long cross-slot operation holds the registry, in a test; no wrkslots command takes a lock outside its own control directory; the lock order is documented and checked.

# Notes

Owner request (via a consumer's lead); filed by the coordinator.
