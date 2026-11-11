---
title: 'validate-10min: a ~10 minute validate on a hot host'
status: open
priority: 1
issue_type: task
assignee: mu-gatefast
created_at: 2026-11-10T22:00:00+00:00
updated_at: 2026-11-10T22:00:00+00:00
---

# Description

[watermelon-full-preview] Owner reply 344 (2026-11-10): validate should be fast
(~10 min) and concurrent; no gate needs the whole host. Measurement on
2026-11-10 from gate logs and the ledger: wrkviz Python shards ran ~700 s
median each at 2 shards, so one shard alone exceeded a 10-minute wall.
Lifecycle namespace shards (~450-570 s), rust.merge-queue.test (~555 s) and
wrkslots shards (~400 s) are the next critical-path steps.

Also here (coordb, 2026-11-10): gate-phase priority must not let queued
exclusive phases outrank owner-tier gates. Nine old-tree cross-differential
exclusive phases starved owner gates because waiters from trees that
predate the owner tier sort by the phase rerun mark alone.

# Design

- Split wrkviz Python tests from 2 shards to 4 (family-weighted assignment
  in run_pytest_shard; the four shards' union is the old selection,
  326+326+326+325 = 1303 tests).
- gate.py: a queued, unstarted exclusive phase over only the five cross
  differentials disabled by default (#1249 cross-differentials-robust,
  owner reply 346) is stale — reaping drops it and records
  `stale-disabled-phase` in history, and a fresh one is not queued. An
  explicit --rerun, and a phase validate.py marks as a deliberate
  `--cross-differentials` opt-in, are never stale. The owner tier keeps
  sorting ahead of every unstarted phase; running gates are never
  preempted.
- Follow-up (not in the first landing): lifecycle shard splits, cargo
  build caching across gates where safe, and re-measurement against the
  10-minute target.

# Acceptance Criteria

- wrkviz runs as 4 shards whose collected union equals the old 2 shards'.
- A stale disabled phase ticket is reaped, an opt-in phase is not, and an
  owner ticket is granted before an unstarted marked phase (focused tests
  in py/tests/test_gate_admission.py).
- Full gate passes on the landed tree; critical path re-measured and the
  remaining minutes named here.

# Notes

Provisional numbers 1253/1258 were taken on main; renumbered to 1288 at landing. The earlier Part 2 commit renamed a neighbouring lane's
issue file by mistake; that rename was reverted in commit 61a16445.
