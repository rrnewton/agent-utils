---
title: 'wrkslots-test-loop-leak: wrkslots test fixtures leave loop devices and mounts behind'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
- validation
depends_on:
  agent-utils-633: discovered-from
created_at: 2026-10-28T13:58:27.499123302+00:00
updated_at: 2026-10-28T13:58:27.499123302+00:00
---

# Description

[opus 5.5] Moved from the coordinator's private backlog by #633 backlog-to-tracker (entry dated 2026-10-26). On the development host, 109 loop devices (44 with deleted backing files) and 228 temporary-volume and pytest mounts had been left by wrkslots test fixtures. Wanted: every fixture that attaches a loop device or mounts a volume detaches and unmounts in teardown, including on failure, and a test proves a failing case leaves nothing; then clean up the leaked ones after checking who owns each. Check first whether later wrkslots work already fixed the fixtures.
