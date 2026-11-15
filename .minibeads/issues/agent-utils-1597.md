---
title: 'proccpu-breach-breakdown: record the PIDs and per-PID ticks behind a budget-crossing CPU sample'
status: open
priority: 2
issue_type: bug
external_ref: gh-234
created_at: 2026-11-15T17:47:04.000000000+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

An unboxed step whose workload uses 0.05 CPU-seconds was charged 10.7 CPU-seconds by the procfs process-group floor and killed as a CPU timeout, in a loaded run that also printed "owner generation gone". The charge never reproduced on idle or loaded reruns, and the failing run's log is not retained, so whether ticks were misattributed (owner-generation path, or session-cache trust under PID reuse at high fork rates) is unknown. The reporting fixture no longer declares a CPU budget; any other unboxed step under the floor remains exposed. Root cause is explicitly unestablished: this issue is the diagnosability step the report asks for, not a claim about the cause.

# Design

Record, with each procfs CPU sample that crosses a step's budget, the member PIDs and per-PID ticks that composed it (and whether each came from live stat or waited-child rollup), in both editions, bounded like the existing member cap. Surface it in the scheduler's budget-breach output so the next occurrence is diagnosable instead of a bare number. Audit the "owner generation gone" to later-sample path and the session-cache reuse checks while there.

# Acceptance Criteria

- A planted snapshot crossing a budget yields a breach record naming every contributing PID and its ticks, summing to the reported total (fixture test, both editions).
- Non-crossing samples record nothing extra.
- The audit's outcome for the two suspect paths is written into this issue (fixed, or shown unable to misattribute, with the test that proves it).

# Notes

[watermelon-full-preview] Filed from GitHub #234 proccpu-breach-breakdown (https://github.com/rrnewton/agent-utils/issues/234) during Request 098 triage. Distinct from #1595 proccpu-malformed-stat-skip (scan aborts, not over-charging).
