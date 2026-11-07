---
title: 'gate-flake-wrkslots-proc-scan: wrkslots shard-0 and shard-2 fail on the 30-second /proc-scan bound under load'
status: closed
assignee: mu-flakes
priority: 1
issue_type: bug
created_at: 2026-11-06T14:30:00.000000000+00:00
updated_at: 2026-11-06T23:40:00.000000000+00:00
closed_at: 2026-11-06T23:40:00.000000000+00:00
---

# Description

python.wrkslots.shard-0 (4) and shard-2 (5) failed first-run gates in about 24 h and passed when rerun alone. The failures include '/proc scan after the path fence exceeded its 30-second bound' in Rust differential fixtures. The shard-2 OOM kills are a separate problem (#883).

# Design

Find what the 30-second bound protects. Make the scan cheaper (restrict it to the fence's processes or cgroup) or make the bound proportional to the process count, keeping the safety check meaningful.

# Acceptance Criteria

- 20 consecutive runs of shard-0 and shard-2 pass under bounded contention.
- A real fence violation is still detected.

# Notes

Filed 2026-11-06 9:30 AM ET at the lead coordinator's request, from one host's gate ledger and disk survey.

## Resolution

Three load-sensitive wall bounds, three fixes, assertions intact:

- The 30 s /proc-scan bound now bounds the scan thread's CPU time
  (user+system, read from /proc/self/task) with an 8x wall outer bound;
  when the 60 s remove-item budget is the limiter the wall bound stays
  exactly the item budget, so registry-lock protection is unchanged.
  A fence violation is still detected (existing holder tests green).
- The audit wrong-row defect split off as #990
  wrkslots-audit-wrong-row-unknown (landed separately): audit retries a
  churn-discarded census inside a 120 s window before stamping UNKNOWN.
- The two real-host liveness tests keep every assertion (including the
  exact PID-namespace stderr lines); their retry deadline now scales
  with measured attempt duration (each transient refusal earns two more
  attempts' worth of time, 600 s overall cap) instead of a flat 60 s.

Contention evidence (#905 method; stress --cpu 6 in bounded scopes):
liveness pair 20/20 (CPUQuota=200% scope); audit own-record test 48/48
under 8-way parallel load (was ~10% failures) and its deterministic
retry test fails pre-fix; shard-0/shard-2 differential remove set:
19/20 in an 8-core dagrun box — the one failure and the 2-core scope
failures (before 0/1, after 1/2) are all the separate 60 s remove-item
wall budget expiring during pre-deletion work on unchanged and fixed
code alike. That budget guards the 110 s heartbeat lock wait and is
tracked by #954 wrkslots-remove-batch-salvage-budget; its measurements
are appended there. It is deliberately not changed here.
