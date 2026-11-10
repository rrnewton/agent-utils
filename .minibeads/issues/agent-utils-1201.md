---
title: "batch-grouped-fencing: remove-agent-batch fences items in groups with one fenced /proc scan per group"
status: open
priority: 3
issue_type: feature
created_at: 2026-11-10T10:30:00.000000000+00:00
updated_at: 2026-11-10T10:30:00.000000000+00:00
---

# Description

After #1175 census-single-pass, Python `remove-agent-batch` makes one multi-target /proc walk per capture before
the fences, but still one fenced /proc scan per item after that item's fence (by path and by dev/ino). So a batch of
N items costs about N+1 walks plus lsof. On a large host (thousands of processes, 100+ slots) the per-item fenced
scans dominate.

# Design

DESIGN FIRST, before any code: fence the items in groups and make ONE fenced scan per group (by path and dev/ino for
every fenced path in the group), then delete each item. The design must settle:
- the per-item order today (fence, scan, delete) versus grouped order, and why grouped is still safe (a process
  that appears between a group's scan and an item's delete);
- budgets: the per-item item budget versus a group budget;
- interruption and recovery: a crash with several items fenced, and what the fence journals record;
- how a refusal for one item affects the rest of its group.
Get the lead's approval on the design before implementing.

# Acceptance Criteria

- A written design, approved by the lead, in Notes.
- Then: a differential showing identical per-item verdicts to the per-item fenced scan, crash-recovery tests with
  several items fenced, and a walk count of about 1 + groups.

# Notes

Split out of #1175 census-single-pass (option B) by the lead coordinator (2026-11-10).
