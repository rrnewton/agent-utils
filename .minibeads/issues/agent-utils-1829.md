---
title: 'wrkslots-cgroup-liveness-design: rebuild liveness on a nested cgroup hierarchy with bounded command paths'
status: open
priority: 1
issue_type: task
labels:
- wrkslots
created_at: 2026-11-18T03:45:00+00:00
updated_at: 2026-11-18T03:45:00+00:00
---

# Description

[watermelon-full-preview] Owner request via the lead coordinator: wrkslots needs a rebuild designed for algorithmic efficiency, not patches. Liveness from a hierarchy of nested cgroups (a slot belongs to its owner's cgroup; empty cgroup = dead owner); no global process or filesystem crawl on any command path; scans only as a cached, non-blocking extra signal.

Motivating failure set: the remove race (#1807); config-lock contention #1276/#1808 (project-root flock free in 35 of 315 samples under about 55 heartbeating lanes, every contender polls non-blocking); the status hang #1827 (unbounded stats into dead FUSE mounts, unbounded subprocess waits, liveness pool join); /proc scans burning the 60 s remove budget (#1802/#1772); reader skew from new persisted fields and the pinned-reader problem #1788 (reader_floor/writer_ceiling); PF_EXITING /proc fd races.

This issue is DESIGN ONLY. No product code.

# Design

Design doc: `ai_docs/plans/2026-11-18-wrkslots-cgroup-liveness.md` covering (1) liveness model — owner cgroup created by the launcher scope, recorded at create/adopt, slot cgroup as child, dead decided by `cgroup.events` populated=0 plus existence, with owner restart, takeover and no-delegation edge cases; (2) command paths with bounded cost O(1)/O(slots), never O(processes)/O(filesystem), scans only as a cached background signal; (3) locking — dedicated config lock file, blocking waits with deadlines, short holds folding #1276/#1808; (4) data and compatibility — additive omit-when-absent fields, staged readers-first then writers via #1788 writer_ceiling, paired-move plan for consumers; (5) staged implementation plan with ETA per stage and which issues each stage closes; (6) risks and alternatives considered.

# Acceptance Criteria

- Design doc at `ai_docs/plans/2026-11-18-wrkslots-cgroup-liveness.md` covers all six sections above.
- Doc validates with hygiene checks only (`make validate` path-selected for the doc); no product code changed.
- A 10-line summary is sent to the coordinator when the doc is ready.

# Notes

Design only; implementation stages in the doc start only on the coordinator's go. Target: doc ready by midday ET on 2026-11-18.
