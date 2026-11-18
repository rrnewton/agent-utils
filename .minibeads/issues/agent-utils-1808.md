---
title: 'wrkslots-create-starves-under-heartbeat-churn: dedicated config lock with blocking deadlines'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- lock-scope
created_at: 2026-11-18T10:20:00+00:00
updated_at: 2026-11-18T10:20:00+00:00
---

# Description

[watermelon-full-preview] Stage 1 of the #1829 wrkslots-cgroup-liveness plan (`ai_docs/plans/2026-11-18-wrkslots-cgroup-liveness.md` section 3), together with the open remainder of #1276 adopt-lock-narrow.

Consumer evidence, 2026-11-18 morning, one shared host with about 140 registry rows and about 55 heartbeating lanes: every `wrkslots create` loses the project-root flock. A validate create was refused twice ("stale base", then "configuration lock busy" after more than 10 minutes) and three agent-slot creates waited about 15 minutes. Measured back to back from 09:41Z: a post-land poll runs one wrkslots operation every 60 s holding the lock 25-30 s, plus heartbeat rounds, a retention pass, and validate admissions (one held 224 s). On 2026-11-17 a create was refused "registry is busy ... 5 lock-free validations" and `/proc/locks` sampling showed the lock free in only 35 of 315 s.

Measured on a synthetic 6-slot fixture (2026-11-18, this lane): the configuration lock is an exclusive `flock` on the project root directory itself. `heartbeat`, `hold` and `unhold` each hold it exclusively for their whole command, running the fleet validation twice plus the registry storage scan under it; `status` takes no exclusive config lock. One heartbeat changes exactly two registry-identity entries (the ACTIVE shard replacement and one appended EVENTS file), which is enough to burn one of a create's 5 lock-free validation rounds, so heartbeat churn starves `create`/`adopt` before the lock wait even starts. `--wait-lock` defaults to 0 (fail-fast poll-refuse), so contenders refuse instead of queueing.

# Design

Dedicated config lock with blocking deadlines, per plan section 3 stage 1, plus the heartbeat half of #1276:

- A single lock file `<control>/config.lock` (mode 0600, created by `init`) replaces the project-root directory as the configuration/registry mutation lock object. The project directory is never flocked by wrkslots again once the transition completes.
- Paired move: upgraded code takes BOTH the legacy project-directory flock and `config.lock`, in that fixed order, until every wrkslots process is upgraded; legacy code takes only the old lock, so old and new stay mutually exclusive through the old lock and new with new through both. The held-lock delegation protocol (`WRKSLOTS_HELD_CONFIG_LOCK_FD`) is re-pointed at the new file unchanged in shape. No new persisted record field in this stage.
- Contenders block on the lock bounded by `--wait-lock` (kernel queue) instead of poll-refusing; expiry reports busy naming the holder.
- `heartbeat` (and the other single-record mutations as converted) validate lock-free and hold the exclusive lock only for the identity compare, the one record write and the fsync, counted not timed. A heartbeat no longer invalidates a create's prevalidation: create/adopt re-check only the slot-specific facts under the lock when the registry changed only by another slot's heartbeat.

Both editions (Python and Rust wrkslotsd) agree on the lock object, the wait discipline and the verdicts, with differential tests. Tests are condition-based only (owner rule 346): they count operations under the lock and wait on files/barriers, never on wall-clock budgets.

# Acceptance Criteria

- Mutations flock `<control>/config.lock`, not the project directory; a Python holder blocks a Rust waiter and vice versa (differential test).
- During transition an upgraded and a legacy process stay mutually exclusive; the consumer upgrade order is written down and sent to the coordinator before gating.
- A heartbeat held under churn no longer makes `create` refuse as busy: create succeeds after heartbeat-only registry changes without burning its bounded validation rounds on them (deterministic regression test, both editions where the surface exists).
- Nothing fleet-wide runs under the exclusive config lock for heartbeat: counted validations/storage scans under the lock are zero (existing counted-not-timed test shape).
- Blocking wait with a caller deadline: a waiter queues behind a short mutation instead of refusing, and expiry names the holder; condition-based tests only.
- Existing lifecycle, lock-scope, prevalidated-mutation and Rust differential suites pass.

# Notes

Coordinates with #1807 (remove race fix, same lock code): implementation rebases on top of #1807 once it is on main. Measurement report sent to the lead coordinator before any code change (2026-11-18).
