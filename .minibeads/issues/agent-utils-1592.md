---
title: 'dagrun-scheduler-memory-admission: gate step starts on live memory headroom instead of the static worst-case sizer'
status: open
priority: 2
issue_type: task
external_ref: gh-113
created_at: 2026-11-15T17:47:04.000000000+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

run_dag and run_dag_limited reach only the static planning-time sizer (py/dagrun/sizing.py jobs_for_budget), so the single worst reachable step combination serializes a whole DAG. The adaptive live-headroom gate in py/dagrun/admission.py is reachable only through the dagrun CLI, never from the scheduler. Reported measurement: a 121-step DAG with one 30 GiB step ran fully serially against a 37 GiB budget. On current main py/dagrun/scheduler.py still imports only the static sizer helper, admission runs once before the CLI run, and run_dag(jobs=) still conflates the step and CPU bounds.

# Design

Gate per-step starts in both schedulers on live headroom (admission: MemAvailable minus margin, re-read per decision, cross-process reservation ledger), so a large-cap step delays only the peers it overlaps. Keep the static sizer as a planning bound, not the runtime concurrency control. Rust and Python editions change together.

# Acceptance Criteria

- In both editions, a DAG whose one heavy step exceeds half the budget still overlaps its light steps (wall time strictly below the sum of step durations in a fixture).
- A step is never started when its reservation exceeds live headroom.
- Rust/Python parity; run_dag(jobs=) either warns or its combined-limit nature is explicit at the call site.

# Notes

[watermelon-full-preview] Filed from GitHub #113 dagrun-scheduler-memory-admission (https://github.com/rrnewton/agent-utils/issues/113) during Request 098 triage. Per-step cgroup boxing may be unavailable in the reporting environment, which is what made the static bound's cost visible.
