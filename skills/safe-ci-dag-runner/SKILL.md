---
name: safe-ci-dag-runner
description: Run a DAG (dependency graph) of CI/build steps concurrently under per-step cgroup CPU/memory boxing, with memory-aware concurrency, learned-estimate planning, and always-on resource logging. Use when scheduling build/test steps by dependencies and resource caps, sizing parallelism to a RAM budget, or profiling a step's parallel speedup.
---

# safe-ci-dag-runner

Runs a DAG of CI/build steps concurrently and safely: dependency- and resource-aware scheduling,
two-level cgroup-v2 per-step CPU/memory boxing, memory-aware `-j`, learned-estimate planning, and
per-step/whole-run resource logging. Python and Rust builds are behavior-identical (proven by a
cross-differential).

The CLI is the source of truth for usage — do not rely on this file for details. Run:

- `safe-ci-dag-runner quickstart` — self-contained getting-started tour (no repo needed).
- `safe-ci-dag-runner --help` — commands and flags.
- `safe-ci-dag-runner --userguide` — the full user guide (complete reference).
