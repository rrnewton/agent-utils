---
name: safe-ci-dag-runner
description: Run a dependency graph of CI or build steps under CPU, memory, and named-resource limits. Use when steps can run concurrently but require dependency ordering, resource boxing, memory-aware parallelism, or measured-speedup planning.
---

# safe-ci-dag-runner

Use this command to run a DAG of build or CI steps with dependency-aware scheduling, cgroup-v2
resource boxing, memory-aware concurrency, learned estimates, and resource reports.

The installed CLI is the source of truth:

- `safe-ci-dag-runner quickstart`
- `safe-ci-dag-runner --help`
- `safe-ci-dag-runner --userguide`
