---
title: 'image-slot-test-time: the wrkslots component tests overrun their 600 s validation step inside an image slot'
status: closed
priority: 3
issue_type: bug
labels:
- wrkslots
- validation
created_at: 2026-10-14T18:04:05.797795834+00:00
updated_at: 2026-10-15T19:34:17.405641449+00:00
closed_at: 2026-10-15T19:34:17.405641309+00:00
---

# Description

[opus 5.5] Found dogfooding a harness whose slots are ext4 disk images (kernel loop mounts). On 2026-10-14 under heavy host load, python.wrkslots.test (scripts/run_component_tests.py --component wrkslots) took 680 s inside an image slot (675 passed, sys time 4m33s) and timed out at the step's 600 s limit during make validate; the same suite took 557 s on a plain directory earlier that day. Measure where the time goes (the suite creates many Git worktrees, processes and small files, so it is metadata-heavy) and either make the suite cheaper or give the step a limit that holds on image-backed slots, so validation inside a wrkslots image slot is not flaky.

# Acceptance Criteria

make validate passes python.wrkslots.test reliably when run inside an image-backed slot on a loaded host; the measurement is recorded.
