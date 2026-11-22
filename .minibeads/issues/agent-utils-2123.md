---
title: 'watch-agents-background-shell-liveness: the background-shell detector reports live children as dead'
status: open
priority: 2
issue_type: bug
labels:
- watch-agents
created_at: 2026-11-22T12:05:00-05:00
updated_at: 2026-11-22T12:05:00-05:00
---

# Description

watch-agents reported that a Muse lane had two background shells and no process under its harness, while both were alive: two long-running children under a live parent process, more than an hour old. The detector liveness check missed live children.

# Design

Find why the liveness check misses them (process tree walk depth, a reparented or sandboxed child, a pid namespace, or a stale snapshot) and fix it; the detector must only report a dead background shell after positive evidence that its process is gone.

# Acceptance Criteria

- A test with a live child under a nested parent is not reported; a really dead one is.

# Notes

Reported by a consumer's lead; filed by the coordinator.
