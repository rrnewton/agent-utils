---
title: 'wrkslots-heartbeat-fallback-scan-under-lock: the heartbeat busy fallback runs the registry storage scan and fleet validation under the registry locks'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-26T23:54:00-05:00
updated_at: 2026-11-26T23:54:00-05:00
---

# Description

When every lock-free round of `wrkslots heartbeat` is overtaken by other writers (dozens of lanes renewing or re-binding at once, as after a reboot), the command falls back to its locked body. That body still runs the whole-fleet validation twice and the registry storage scan (every slot's storage state, unregistered worktrees, Git paths without a row) while it holds the exclusive ACTIVE registry locks. #2000 moved the configuration lock out of the way, but the registry locks are still held for work that grows with the registry, so a storm of heartbeats serialises behind each fallback and other lanes' heartbeats and creates wait or time out. This is the same defect class as #2174 wrkslots-transfer-owner-scan-under-config-lock, which fixes the transfer-owner fallback.

# Design

Give the heartbeat fallback the treatment #2174 gives transfer-owner. Under the locks run only the journal phase, the touched record's own checks (owner authorisation, validate-batch seals and cleanup journals that concern this record, its release state from its machine's event chain, and its own storage check), the write, and a snapshot comparison proving no other local row moved. The fleet validation and the registry storage scan stay in the lock-free prepare, and the busy fallback reuses the last lock-free round's findings (the tolerated nested-worktree warnings) instead of recomputing them under the lock. Check that each record-scoped check that currently takes the fleet states (for example the validate cleanup-journal check) can be answered from the local state, or from facts the lock-free round gathered, without a fleet read under the lock. The Rust edition's heartbeat must behave the same way.

# Acceptance Criteria

- The heartbeat busy fallback runs no fleet validation and no registry storage scan while it holds the registry locks, proven by a hermetic test in which the lock-free storage scan runs a second writer with a zero lock wait and that writer acquires the locks (a condition, never a wall-clock budget); the test is red on the current fallback.
- The touched row is still fully judged under the lock: a storage defect in that row introduced after the lock-free scan still refuses the heartbeat, and a released generation still refuses.
- Python and Rust parity, covered by deterministic regression tests.

# Notes

Found while completing #2174 wrkslots-transfer-owner-scan-under-config-lock, whose locked fallback had the same full scan under the lock. To be fixed in the same lane after #2174 lands.
