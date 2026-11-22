---
title: 'wrkslots-cgroup-scoped-run-liveness: decide a run''s liveness from its cgroup, not a host-wide process scan'
status: open
priority: 1
issue_type: feature
labels:
- wrkslots
created_at: 2026-11-22T06:05:00-05:00
updated_at: 2026-11-22T06:05:00-05:00
---

# Description

The owner: "The scan of a continuously mutable large entity non-atomically will never work." A validation run's own processes should be scoped by its cgroup rather than found by scanning every process on the host.

# Design

Use the run's cgroup: cgroup.events populated 0 as the liveness read, and cgroup.kill for tearing down the subtree; per-run PID namespaces are acceptable as an alternative. Process listing is used only to name a holder in a refusal, never to decide.

# Acceptance Criteria

- Run liveness and teardown never enumerate host processes; tests with a fake cgroup tree.

# Notes

Filed by the coordinator from owner directions relayed by a consumer's lead; the owning lane refines this file as it works.
