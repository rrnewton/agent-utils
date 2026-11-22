---
title: 'wrkslots-recover-linear-work-under-lock: move the recover-path censuses and slice probe outside the mutation locks'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
created_at: 2026-11-22T23:05:00-05:00
updated_at: 2026-11-22T23:05:00-05:00
---

# Description

Seal recovery runs entirely under the registry mutation locks. On that path the B2-B slice probe walks all of /sys/fs/cgroup (about 0.9 s measured, about 2053 directories) and the all-process and same-UID censuses enumerate host processes, all while holding the locks. The owner rule is constant-time work under a lock, with linear work prepared outside and validated under the lock with an O(1) generation check. #2128 kept the existing behaviour on this path because moving it changes the teardown closure; this issue is the follow-up.

# Design

Prepare the censuses and the slice-subtree answer before taking the mutation locks, then validate under the locks with an O(1) generation check (record generation, seal journal identity, fence identity) before any destructive phase. No new persisted field; the prepared evidence names what it covered so a change after preparation refuses instead of deleting on stale evidence.

# Acceptance Criteria

- Recovery under the mutation locks performs no /sys/fs/cgroup walk and no host process enumeration; focused tests show a recovery completing with the same refusals as today (live holder, unreadable scope, changed generation) while unrelated churn runs.

# Notes

Filed from the #2128 cold reviews. The scoped cgroup decision itself is O(1) and is not the subject here; only the pre-existing linear work on the recover path is.
