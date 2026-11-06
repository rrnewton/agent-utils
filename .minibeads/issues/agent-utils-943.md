---
title: 'gate-flake-wrkslots-proc-scan: wrkslots shard-0 and shard-2 fail on the 30-second /proc-scan bound under load'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-06T14:30:00.000000000+00:00
updated_at: 2026-11-06T14:30:00.000000000+00:00
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
