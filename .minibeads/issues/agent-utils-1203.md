---
title: "cross-skip-accounting-memory-cap: the cross-skip-accounting step is intermittently OOM-killed at its inner memory cap"
status: closed
priority: 2
issue_type: bug
assignee: cl-census
created_at: 2026-11-10T11:20:00.000000000+00:00
updated_at: 2026-11-10T11:50:00.000000000+00:00
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

## Resolution

Measured on devbig030 with sequential solo dagrun runs (cl-census, opus 5.5). dagrun's inner cap is
`rss_baseline_bytes` x 1.25 when a slot's `.dagrun/profiles` has no larger recorded estimate, so a fresh slot ran
this step at 256 MiB x 1.25 = 320 MiB, while slots with history got 346-471 MiB. That is why it failed 3/3 in one
slot and passed in another.

- `cross-skip-accounting`: 20 runs at the old baseline gave peaks of 239-377 MiB (memory.peak of the step's cgroup)
  under derived caps of 385-471 MiB. One run peaked at 377 MiB under a 385 MiB cap. Baseline now 576 MiB, so the cap
  is 720 MiB with no profile history (about 1.9x the 377 MiB peak). Acceptance: 20/20 consecutive solo passes with
  `--no-profile-feedback` (cap 720 MiB; peaks 240-450 MiB, mean 310 MiB). The peak grows with the cap because
  page cache fills the room, so it does not count against the margin.
- Siblings on the 256 MiB baseline (5 runs each): `validation-graph` was OOM-killed at 320 MiB in its first run (peaks
  211-398 MiB), so its baseline is now 640 MiB (cap 800 MiB; 10/10 without profile feedback, peak <= 322 MiB).
  `cli-surface` peaked at 311 MiB against a 320 MiB fresh-slot cap, so its baseline is now 512 MiB (cap 640 MiB; 10/10,
  peak <= 304 MiB). `cli-no-optional-deps` (peak <= 86 MiB) and `cross-environment` (<= 173 MiB) keep 256 MiB.
  `cross-environment-b` to `-f` reach their cap without OOM events: the room is page cache (swap is off, so anonymous
  memory at the cap would be killed). They keep 256 MiB.
- Peak print: `scripts/run_pytest_shard.py` (`_MemoryPeakPlugin`, in both launch modes) prints, on stderr, under a
  finite memory.max only: "run_pytest_shard: memory peak N MiB of the M MiB cap" at the end of a session, and once,
  flushed, "memory N MiB of the M MiB cap after <test>" when the capped cgroup first passes 80% of its cap. That line
  lands in dagrun's failure detail before an OOM kill and names the test that got there. Self-test:
  `_self_test_capped_memory_cgroup`.
