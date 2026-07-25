# safe-ci-dag-runner

Run a **directed acyclic graph (DAG) of CI / build / test steps** concurrently and *safely*:

- **Two-level cgroup boxing.** The whole run gets an outer CPU/memory box; each step gets its own
  nested box. A step that blows its memory budget is capped (or OOM-killed) in isolation, and if a
  step dies or the run is cancelled, its entire process subtree is torn down immediately — **no
  orphaned or zombie processes**.
- **Memory-aware concurrency.** Instead of a fixed `-jN`, the runner reasons about which steps can
  actually co-run (given the dependency graph and per-step memory estimates) and picks the largest
  parallelism that fits the machine's real available RAM.
- **Always-on resource logging.** Every run records per-step CPU/memory and the machine's *ambient*
  load (load average, pressure-stall info, co-tenant build count) so a slow result on a busy box is
  not mistaken for a slow step. Logs are appended even when steps fail.

This is one tool from [`agent-utils`](https://github.com/rrnewton/agent-utils); it ships as both a
Python package and a Rust crate with identical behavior (verified by differential tests).

> Status: 🚧 early. The API below is the target shape and is still settling.

## Concepts

- **Step** — one node: a shell command, its dependencies (by name), and a **resource hint**
  (estimated duration, memory baseline, scarce-resource demands, internal parallelism).
- **DagConfig** — the whole graph plus caller policy: resource capacity caps (e.g. only 2 browser
  steps at once), subset presets, cache-exempt path prefixes.
- **Runner** — schedules ready steps onto a worker pool, honoring dependencies, resource caps, and
  the memory budget; boxes and measures each step.

## Python (reference implementation)

```python
from safe_ci_dag_runner import Step, ResourceHint, DagConfig, run_dag

cfg = DagConfig(
    steps=[
        Step("build", "app", "compile", "make build",
             hint=ResourceHint(rss_baseline_bytes=2 * 1024**3, est_duration_s=90)),
        Step("test", "unit", "unit tests", "make test", deps=["build.app"],
             hint=ResourceHint(rss_baseline_bytes=4 * 1024**3, est_duration_s=120)),
    ],
    resource_caps={"browser": 2},
)
result = run_dag(cfg)          # returns a RunResult; nonzero on any step failure
```

CLI:

```sh
safe-ci-dag-runner --list                 # show the DAG
safe-ci-dag-runner --only test.unit       # run one step (+ its deps)
safe-ci-dag-runner --dot | dot -Tsvg > dag.svg
```

## Rust

```toml
# Cargo.toml
[dependencies]
safe-ci-dag-runner = "0"
```

```rust
use safe_ci_dag_runner::{DagConfig, ResourceHint, Step, run_dag};
```

The Rust binary exposes the same CLI as the Python one.

## Platform notes

The cgroup boxing is **Linux cgroup-v2** (systemd-user delegated scope preferred, with a
systemd-free delegated-cgroupfs fallback). On hosts without delegated cgroups the runner degrades
gracefully to process-group teardown and reports reduced enforcement loudly (it never silently drops
a cap). The DAG model, scheduler, and ambient logging work everywhere; exact per-step CPU/mem
accounting needs a delegated cgroup-v2 host.

## License

MIT.
