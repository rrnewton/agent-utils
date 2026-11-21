---
title: 'wrkslots-orphan-loop-detach: a sanctioned detach for a loop mount whose backing image was deleted'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T11:45:00-05:00
updated_at: 2026-11-21T11:45:00-05:00
---

# Description

A test teardown deleted slot images while they were still loop-mounted. imagehelper's umount verifies the image by device and inode, which no longer exist, so the loop devices are stranded with no sanctioned way to detach them.

# Design

Add `wrkslots image detach-orphan --device --image --mount-point`: act only when the backing file shows deleted, its path is exactly the expected slot image under this registry, and the device is mounted exactly at the expected point; refuse anything else, refuse a busy mount without lazy unmount, and print exactly what was acted on.

# Acceptance Criteria

- The behaviour described under Design, covered by a deterministic regression test (condition-based, no wall-clock budgets).

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
