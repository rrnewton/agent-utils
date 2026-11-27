---
title: 'wrkslots-transfer-owner-scan-under-config-lock: transfer-owner holds the configuration lock through a full registry storage scan'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-24T03:00:00-05:00
updated_at: 2026-11-27T12:13:29-05:00
closed_at: 2026-11-27T12:13:29-05:00
---

# Description

`wrkslots transfer-owner` holds the configuration lock (worktrees/config.lock, then .wrkslots.yml.lock) for minutes per call, because it runs the full registry storage scan (every slot's storage state, unregistered worktrees, Git paths without a row) while holding it. In a post-reboot re-bind with about 30 lanes transferring at once, --wait-lock 600 callers timed out with "REFUSED: configuration lock is busy"; one holder kept config.lock for over 9 minutes. Only constant-time work may run under the registry locks, so a linear registry scan under the lock breaks every other lane's heartbeats and creates.

# Design

A dead-owner transfer touches one row. Move the registry storage scan out from under the configuration lock (or skip it for transfer-owner, re-checking only the transferred row under the lock), so the locked section is constant-time. Apply the same treatment to any other single-row command that runs the same scan under the lock. Python and Rust editions behave identically.

# Acceptance Criteria

- transfer-owner no longer holds the configuration lock during the registry storage scan, proven by a hermetic test where a scan that blocks on a condition (never a wall-clock budget) does not block a second command's lock acquisition.
- The transferred row is still fully validated under the lock; no refusal the scan produced for that row is lost.
- Python and Rust parity, covered by deterministic regression tests.

# Notes

Evidence is from a consumer's post-reboot re-bind (about 130 slots scanned per call); the scan findings themselves ("missing or inconsistent storage. state: RETAINED", unregistered worktrees, Git paths without a row) are registry-wide and do not concern the one transferred row.

## Resolution

[Claude Opus 5.5] Landed on main as e5420109 + 3f77daee (full gate PASS 71/71 on 8b141e43). The transfer-owner busy fallback no longer runs the registry storage scan or fleet validation under the registry locks. Under the locks it checks only the transferred row's own storage. Its nested registrations come from the last lock-free prepare, or from a one-slot scan when no prepare finished. Tests in test_transfer_lock_scope.py: a second writer runs from inside the scan and never blocks (red on the first commit alone); a slot broken after the scan is still refused under the lock; the one-slot nested registrations equal the full scan's for that slot; the locked fallback counts zero storage scans under the lock. The Rust edition already judged only the record under the lock. No persisted fields. Heartbeat's identical fallback is #2353.
