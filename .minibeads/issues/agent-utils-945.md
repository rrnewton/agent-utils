---
title: 'merge-queue-regenerate-conflicts: resolve conflicts confined to declared generated paths by regenerating them'
status: open
priority: 1
issue_type: task
labels:
- merge-queue
created_at: 2026-11-06T14:09:26.584874758+00:00
updated_at: 2026-11-06T14:25:36.006714516+00:00
---

# Description

[watermelon-full-preview] A consuming queue's main throughput limit: after one landing, most ready changes conflict only in derived files the repo regenerates, each costing a manual rebase/regeneration/re-review cycle. Declare generated paths (globs) in the queue config (top-level generated_paths, merged into regenerate.paths); when composing a candidate conflicts only in declared generated paths, settle those paths on the base side, run the configured regenerate command, validate that tree, record a regenerated-conflict event naming the paths, and land exactly the validated tree. Mixed conflicts, regenerate failure, conflict markers left behind, or regeneration touching non-generated paths still refuse; --exact changes are never rewritten. merge-queue compose --check <PR> is a read-only dry run partitioning conflicts into auto-regenerated vs needs-a-human.

# Acceptance Criteria

Tests cover: only-generated conflict resolved/regenerated/validated with event and pushed tree == validated tree; mixed conflict refused; regenerate failure refused; regenerate touching non-generated paths refused; --exact never rewritten; compose --check output partitions conflicts.
