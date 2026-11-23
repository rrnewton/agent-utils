---
title: 'wrkviz-export-temp-dirs-leak: a stopped archive export leaves its uncompressed temporary directories behind'
status: open
priority: 2
issue_type: bug
labels:
- wrkviz
created_at: 2026-11-22T21:05:00-05:00
updated_at: 2026-11-22T21:05:00-05:00
---

# Description

The combined archive export renders every team, uncompressed, into a host temporary directory (a Python TemporaryDirectory under the system temp dir), and the normalize step uses mkdtemp. When a run is stopped (a signal, a disk-floor stop, a scope kill), those directories are never removed: two decompressed export directories from earlier stopped runs were found left behind on a host, each holding a full uncompressed render.

# Design

- Put every temporary render under the run's own state directory (or honour TMPDIR) so it is counted against the run's disk budget and removed with it.
- Clean up on SIGTERM and SIGINT, and sweep stale render directories from earlier runs (by owner process liveness) at the start of the next run.
- Report the bytes reclaimed.

# Acceptance Criteria

- A test stops an export mid-render with SIGTERM and finds no temporary directory left behind.
- A second run removes the leftovers of an earlier killed run.

# Notes

Reported by a consuming team running the archive rebuild under disk watchers; filed by the coordinator.
