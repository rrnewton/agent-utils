---
title: 'wrkslots-transfer-differential-flake: the transfer-owner differential fails about 1 in 9 under host load'
status: open
priority: 3
issue_type: task
labels:
- wrkslots
created_at: 2026-11-02T16:26:35.708411010+00:00
updated_at: 2026-11-02T16:26:35.708411010+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). test_transfer_unowned_matches_python (test_transfer_owner_write_rust.py) fails ~1/9 under concurrent host load (mu-rust, 2026-10-28 slices 40-41, cl-refactor 4a gate); passes alone. Make the differential deterministic (isolate timing/event ordering).
