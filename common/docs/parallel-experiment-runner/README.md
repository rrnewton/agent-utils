# parallel-experiment-runner

Run concurrent seed-sweep workers under `safe-ci-dag-runner` resource containment.

The command calibrates concurrency, maps each seed to one boxed DAG step, enforces
CPU, memory, PID, and wall-time limits, and reports measured per-round outcomes.
See the bundled user guide for the complete command and profile-key contract.
