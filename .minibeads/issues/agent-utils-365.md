---
title: 'package-step-cpu-budget: the Python package steps run on the 10 CPU-second default and time out on a hosted runner'
status: closed
priority: 2
issue_type: bug
labels:
- validation
created_at: 2026-10-19T09:35:55.616000892+00:00
updated_at: 2026-10-19T09:43:57.368854363+00:00
closed_at: 2026-10-19T09:43:57.368854213+00:00
---

# Description

[opus 5.5] `packages.python.wrkviz` failed with "CPU-TIMEOUT >10s cpu" in repository-ci (push to main 2026-10-19, and PR runs 2026-10-18), with nothing wrong in the package. The eight Python package steps in `validation/packages.dag.yaml` declare no `cpu_timeout`, so dagrun's default small-step budget of 10 CPU-seconds applies to building a wheel and an sdist and smoke-installing them in a fresh environment, which takes about that much CPU on a hosted runner. #294 (wall-clock multipliers) is related but does not cover CPU budgets.

Fix: give every package step an explicit CPU budget sized for a build plus a smoke install, the way every other build step in the graph declares one.

Acceptance: a test pins that every package step declares a CPU budget above the default, and the package steps pass on a hosted runner.
