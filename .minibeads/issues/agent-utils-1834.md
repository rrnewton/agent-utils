---
title: 'cross-agentctl-profile-drift-wait: editions disagree on ultra profile drift and the differential waits out the disagreement'
status: closed
priority: 0
issue_type: bug
labels:
- agentctl
- cross
created_at: 2026-11-18T04:39:03-05:00
updated_at: 2026-11-18T07:01:50-05:00
closed_at: 2026-11-18T07:01:50-05:00
---

# Description

[watermelon-full-preview] Finding (lead coordinator, 2026-11-18): the agentctl cross differential shard that runs the profile groups takes about 4,100 s on every gate. Cause, reproduced by hand on the watermelon profile case: for `agentctl status worker`, the Python edition reports profile drift on `environment_names` with the restart fix line while the Rust edition reports no drift (both show declared effort ultra, effective effort xhigh). The Python result is the same in a clean environment, so it is deterministic, not host leakage. The differential then polls until both editions could evaluate the profile for up to the 300 s hang guard on every status pair whose only difference is profile drift, and passes anyway because agreement tolerates a drift-only difference. Each such pair burns 5 minutes.

# Design

- Find which edition is right about `environment_names` drift for an ultra-effort Muse profile (the ultra gate environment variable a profile adds implicitly at start) by comparing the Python start, drift and revive paths with the Rust equivalents, and fix the wrong edition so both agree. A real drift report must stay correct for real records.
- Make the differential fail fast instead of waiting out a deterministic disagreement: any remaining drift wait must only cover a genuinely transient state and end on a condition, not by running out the hang guard; if no transient case exists, remove the wait. Keep the hang guard. No wall-clock budgets.
- List every other differential case that hits the drift wait, confirm they are gone, and measure shard time before and after.

# Acceptance Criteria

- Both editions agree exactly on profile drift for the ultra Muse profile, with a deterministic regression test for the edition mismatch in both editions and a differential assertion that profile drift agrees exactly for the ultra profile.
- A real profile change (for example a new profile environment name) still reports drift in both editions.
- The differential no longer waits out a drift-only disagreement; a drift mismatch fails on the first comparison.
- Shard time before and after is measured and reported.

# Notes

Gate speed: aim for about a 10-minute validate. Times in ET.

## Resolution

[watermelon-full-preview] Landed on github/main as `f59daa79c85905e30b809211e198c8466fbf6d1a` (gated head `b96a0bef3b03cb18bfecbda9bcc45c56d0e0445f`, gate exit 0; the gate verdict is PARTIAL only for 7 delegated-live-scope skips in the dagrun/cpuset differentials, unrelated to this change and unverified there as everywhere). Root cause: the Python status drift check computed profile environment names without the ultra gate name start synthesizes for a muse ultra launch, so it reported `environment_names` drift the Rust edition correctly omits; the differential then polled out the 300 s hang guard on every such status pair and passed anyway. The Python check now mirrors the Rust refreshed-profile comparison, status pairs compare exactly on first read (the wait and the drift-only tolerance are gone; the hang guard still bounds each invoke), and both editions plus the differential pin the agreement with regression tests, including that a real profile environment change still reports drift. The eight watermelon status pairs that each burned 300 s now take about 2.5 s each; shard `1/3` went from 4,197 s (2026-11-17 gate) to 775 s standalone and 765 s in gate, with 266 paired checks agreeing.
