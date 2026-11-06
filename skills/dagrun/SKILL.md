---
name: dagrun
description: Run a dependency graph of validation or build steps under CPU, memory, and named-resource limits. Use when steps can run concurrently but require dependency ordering, resource boxing, memory-aware parallelism, or measured-speedup planning.
---

# dagrun

Use this command to run a DAG of build or validation steps with dependency-aware scheduling, cgroup-v2
resource boxing, memory-aware concurrency, learned estimates, and resource reports.

The installed CLI is the source of truth:

- `dagrun quickstart`
- `dagrun --help`
- `dagrun --userguide`

A step may declare `priority: N` (integer, default 0): among ready steps the
higher priority dispatches first when slots or resources are short, with the
planner's order as tie-break. It adds no dependency; all-default priorities
keep the previous dispatch order exactly.

A step may declare `publishes: [{name, path?}]` and a dependent may declare
`artifact_deps: [{step, artifact}]` to start before the producer exits: the
producer signals by renaming `<dir>/<step-tag>/<name>.tmp` to
`<dir>/<step-tag>/<name>` under `$DAGRUN_ARTIFACT_READY_DIR` (or runs
`dagrun artifact-ready <name>`), the producer keeps its box and resources
until exit, and a producer that later fails — or exits 0 without signalling —
fails the run and discards released dependents.
