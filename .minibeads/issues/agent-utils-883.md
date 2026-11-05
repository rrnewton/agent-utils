---
title: 'wrkslots-shard-memory-and-heartbeat: shard-0 stays inside its memory cap and the heartbeat-renewal test survives load'
status: open
priority: 2
issue_type: bug
assignee: mu-salvage
created_at: 2026-11-05T11:28:43.362021367+00:00
updated_at: 2026-11-05T11:28:43.362021367+00:00
---

# Description

Two repeat offenders appeared in the gate ledger on 2026-11-05 after #875 fixed shard-6. mu-salvage examined both:

1. **`python.wrkslots.shard-0`, 3 first-run failures in 3 h: OOM-killed.** The step's inner MemoryMax is about 1.9 GiB; there were 5 `oom_kill` events in each failure. It happens under parallel gate contention, and it passes alone in 170-210 s. It is not a test assertion. The shared `wrkslots_test_hint` gives every wrkslots shard `rss_baseline_bytes` 1.5 GiB (validation/python-components.dag.yaml), but shard-0 evidently peaks above that cap when other gates run.
2. **`python.wrkslots.shard-4`, 2 failures: `test_audit_lock_free.py::test_audit_heartbeat_renewal_marks_only_that_row_unknown`.** Slot-b came back UNKNOWN under load and passed alone. This is audit/heartbeat timing; it sits next to the liveness work in #879, but it is not #875's census family.

# Design

- **shard-0:** measure the per-family peak RSS (pytest with a memory probe, or cgroup `memory.peak` per family) and find what drives it.
  - If a family genuinely needs the memory, give shard-0 its own hint with a measured baseline and cap, or move that family to its own shard.
  - If it is a leak or an oversized fixture, fix that instead.
  - Do not raise every shard's cap blindly; #851 set them deliberately.
- **shard-4:** make the heartbeat-renewal test independent of wall-clock load. Use an injected clock or a deterministic renewal point instead of real elapsed time, keeping what it asserts: renewal marks only that row unknown.

# Acceptance Criteria

- shard-0 has no OOM kill in the next 24 h ledger readout, and its cap stays within the gate host's overall memory budget.
- The shard-4 test passes under a load test: for example, 4 parallel copies plus a CPU hog for 20 iterations, or the gate's own stress mode.

# Notes

From mu-salvage's look at the gate ledger on 2026-11-05. Assigned to mu-salvage after #881. If #879 changes the same liveness code, coordinate with mu-validate.
