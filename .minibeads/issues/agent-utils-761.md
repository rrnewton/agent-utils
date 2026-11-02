---
title: 'mb-id-allocation-race: issue numbers chosen by scanning collide between lanes'
status: open
priority: 3
issue_type: task
labels:
- minibeads
created_at: 2026-11-02T16:26:35.756479670+00:00
updated_at: 2026-11-02T16:26:35.756479670+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). lanes pick issue numbers with a scan loop over main + slots; collisions happened 3x on 2026-11-01 (#719 vs validate-speed, #725 twice) because unpublished/rescued issues are invisible and lanes allocate concurrently. Need an atomic allocator (e.g. mb next-id with a lock file in the harness, or reserve-by-commit on main). (2026-11-01)
