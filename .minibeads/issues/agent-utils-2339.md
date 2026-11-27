---
title: 'dagrun-learned-cap-raise-after-zero-test-oom: a step OOM-killed at its learned memory cap before completing any test gets a higher learned cap for its next run'
status: open
priority: 3
issue_type: feature
assignee: cl-shardcap
labels:
- dagrun
- validation
created_at: 2026-11-26T20:33:00-05:00
updated_at: 2026-11-26T20:33:00-05:00
---

# Description

When dagrun OOM-kills a step at its learned memory cap, the run ends NO_RESULT ("a learned-cap breach under contention is not a failure", #1763). The breach isn't counted as a measurement either, so the learned value stays where it was. A step that can't finish under its cap therefore hits the same cap in every later run, until someone re-profiles it by hand.

That's what happened to `python.agentctl.shard-0` and `shard-1` from 2026-11-20 to 2026-11-26. Both were killed at their 2.5 GiB cap at their first Rust case, with no completed tests, in every full gate. The cause (#2306) was a release compile inside the shard that needed about 3 GiB. Nothing raised the cap, and nothing reported the steps (#2317), so about 2900 tests went unmeasured for six and a half days. #2306 moved the compile out, so those shards are no longer affected. The mechanism that let one bad cap persist is still in place for any other step.

This comes from #2253's design (closed as a duplicate of #2306): an OOM kill at a learned cap that is below the step's own declared or recorded need is not contention; the cap simply can't be met.

# Design

- When a step is OOM-killed at its learned cap and its runner reports zero completed tests, dagrun records that outcome in the step's profile, alongside the peak at the kill.
- The next run of that step starts from a higher cap: the step's declared hint cap (`rss_baseline_bytes` x the cap factor) when that is higher than the learned cap, and otherwise the learned cap times a fixed step factor. A configured ceiling, per step or per DAG, bounds it, so a runaway step can't claim the host.
- A step that keeps getting killed after raises reaches the ceiling and stays NO_RESULT there. From that point on, #2317's repeated-NO_RESULT health report is the signal; the cap stops growing.
- A breach after one or more completed tests keeps today's behaviour: contention is a plausible cause there, and the step has already produced measurements.
- The NO_RESULT semantics stay as they are (#1763, owner rule 346): only the next run's starting cap changes.

# Acceptance Criteria

- Test: a step OOM-killed at its learned cap with zero completed tests starts its next run at a higher cap, either its declared hint cap or its learned cap times the step factor, whichever the design picks. It does not start again at the cap it was killed at.
- Test: the raised cap never exceeds the configured ceiling, and a step killed at the ceiling is not raised again.
- Test: a breach after at least one completed test leaves the learned cap unchanged.
- No wall-clock pass/fail budget is introduced (owner rule 346).

# Notes

Low priority: since #2306, no known step hits this. Filed at the lead coordinator's request from #2253's design. Related: #1763 (learned-cap NO_RESULT semantics), #2207 (a learned-cap NO_RESULT run exits 3), #2253 (the original design), #2306 (the agentctl shards case), #2317 (repeated NO_RESULT runs and incomplete suites made visible), #818 (the peak + max(25%, 512 MiB) sizing rule).
