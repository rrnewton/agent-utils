---
title: "cross-skip-accounting-memory-cap: the cross-skip-accounting step is intermittently OOM-killed at its inner memory cap"
status: open
priority: 2
issue_type: bug
created_at: 2026-11-10T11:20:00.000000000+00:00
updated_at: 2026-11-10T11:20:00.000000000+00:00
---

# Description

`python.repository-infrastructure.cross-skip-accounting` (`validation/python-components.dag.yaml`,
`rss_baseline_bytes: 268435456`, classified light, about 3 s) is sometimes OOM-killed at its inner cgroup MemoryMax.
dagrun reports "MEMORY CAP PROBLEM ... OOM-killed at its inner cgroup MemoryMax (cap~320-346 MiB, peak = cap). This is
a cap problem, not a test failure". On 2026-11-10 it failed twice in a #624 gate and its solo rerun (cap ~320 MiB),
and once on unmodified main 2b55c42d (cap ~346 MiB). It then passed 3/3 on the same main, and passed on every main
commit bisected back to 09f29c54. So the step's peak memory sits right at the cap, and the cap (derived from the
256 MiB baseline plus profile data) is too tight.

# Design

Measure the step's real peak RSS across repeated runs (dagrun's profile data, or /usr/bin/time -v around the pytest
command), then set `rss_baseline_bytes` with headroom over the observed maximum (e.g. 1.5x). Check the sibling
repository-infrastructure steps that share the 256 MiB baseline for the same margin problem.

# Acceptance Criteria

- 20 consecutive solo runs of the step pass with the new baseline.
- The Notes record the observed peak, the chosen baseline and the cap dagrun derived from it.
- Sibling steps checked; any others near their cap fixed in the same change.

# Notes

Filed by the devbig030 coordinator (2026-11-10) after bisecting the failure on main. Unassigned.
