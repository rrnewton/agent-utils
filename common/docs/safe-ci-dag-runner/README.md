# safe-ci-dag-runner

Run a **directed acyclic graph (DAG) of CI / build / test steps** concurrently and *safely*.
You describe your steps as shell commands with dependencies (and optional resource hints) in a small
JSON file (or in Python), and the runner schedules them across a worker pool — respecting
dependencies and scarce-resource limits — while boxing each step so a runaway command cannot take
down the run or the host.

## What you get

- **Two-level cgroup boxing.** The whole run can execute inside an outer CPU/memory box, and each
  step gets its own nested box. A step that blows its memory budget is OOM-killed *in isolation* at
  its own cap, and when a step finishes, times out, or the run is cancelled, its **entire process
  subtree is torn down immediately** — including `setsid`/double-forked escapees (orphan servers,
  browsers) that a plain process-group kill misses. No orphaned or zombie processes.
- **Memory-aware concurrency.** Instead of guessing a fixed `-jN`, the runner can reason about which
  steps could actually co-run (given the dependency graph, scarce-resource caps, and per-step memory
  estimates) and compute the largest parallelism that fits a real RAM budget.
- **Always-on resource logging.** A run can record per-step CPU/memory and the machine's *ambient*
  load (load average, pressure-stall info, co-tenant build count), so a slow result on a busy box is
  not mistaken for a slow step. Logs are written even when steps fail.
- **DAG visualization.** Render the graph as Graphviz DOT (`dot -Tsvg`) or compact ASCII for a quick
  terminal glance.

Containment and metrics are **pluggable** (the `CgroupManager` and `MetricsSink` protocols), so the
scheduler runs anywhere: with real Linux cgroup-v2 boxing where available, or with safe no-op
stand-ins (`NoopCgroups`, the default) on any other host.

This is one tool from [`agent-utils`](https://github.com/rrnewton/agent-utils). It ships as a Python
package and a Rust crate; the two are intended to be behaviorally identical, but **today the Python
package is the working implementation** — see [Status & limitations](#status--limitations).

## Install

```sh
pip install "git+https://github.com/rrnewton/agent-utils#subdirectory=py"
```

This installs the `safe-ci-dag-runner` console script and the importable `safe_ci_dag_runner`
package. Requires Python 3.10+ (Linux for the cgroup features; the DAG model, scheduler, and
visualization work anywhere).

For a self-contained tour any time, run:

```sh
safe-ci-dag-runner quickstart
```

## 60-second quickstart

Save a DAG as `dag.json`. Each step has a `group` and a `job` (its tag is `group.job`), a shell
`cmd`, and may depend on other steps by tag:

```json
{
  "resource_caps": {"browser": 1},
  "steps": [
    {"group": "build", "job": "app", "desc": "compile the app",
     "cmd": "echo building && sleep 0.2",
     "hint": {"est_duration_s": 5, "classification": "cpu-bound"}},
    {"group": "test", "job": "unit", "desc": "unit tests",
     "cmd": "echo unit-tests && sleep 0.2", "deps": ["build.app"]},
    {"group": "e2e", "job": "smoke", "desc": "browser smoke test",
     "cmd": "echo e2e && sleep 0.2", "deps": ["build.app"],
     "hint": {"resources": {"browser": 1}}}
  ]
}
```

Look at it, then run it:

```sh
$ safe-ci-dag-runner list --dag dag.json
build.app  [cpu-bound] compile the app
test.unit  [light] unit tests  <- build.app
e2e.smoke  [latency-bound] browser smoke test  <- build.app

$ safe-ci-dag-runner ascii --dag dag.json
DAG - 3 steps, 2 edges, 2 layer(s)

layer 0:
  build.app  [cpu-bound]
layer 1:
  e2e.smoke  [latency-bound] {browser:1}  <- build.app
  test.unit  [light]  <- build.app

$ safe-ci-dag-runner run --dag dag.json
[build.app] ▶ START  compile the app
[build.app] ✓ PASS   compile the app (0s)  [building]
[test.unit] ▶ START  unit tests
[e2e.smoke] ▶ START  browser smoke test
[e2e.smoke] ✓ PASS   browser smoke test (0s)  [e2e]
[test.unit] ✓ PASS   unit tests (0s)  [unit-tests]
safe-ci-dag-runner: PASS - 3 passed, 0 failed, 0 aborted, 0 skipped in 0.5s
```

`build.app` runs first; once it passes, `test.unit` and `e2e.smoke` run concurrently. The exit code
is `0` because every step passed. (The per-step lines go to stdout; the final `PASS`/`FAIL` summary
line goes to stderr.)

## CLI reference

```
safe-ci-dag-runner <command> [options]
```

Every command except `quickstart` takes `--dag FILE` (use `-` to read the DAG from stdin).

| Command      | What it does                                                        |
| ------------ | ------------------------------------------------------------------- |
| `run`        | Run the DAG. Exit `0` iff every step passes.                        |
| `list`       | List the steps with class and dependencies (registration order).    |
| `ascii`      | Draw the DAG as ASCII art, grouped by topological layer.            |
| `dot`        | Emit Graphviz DOT to stdout (pipe to `dot -Tsvg`).                  |
| `json`       | Re-emit the DAG as canonical, fully-defaulted JSON.                 |
| `quickstart` | Print a self-contained getting-started guide (no `--dag` needed).  |

Global: `--version`, `-h/--help`. Running with no command prints help and exits `0`.

### `run` flags

| Flag                 | Meaning                                                                             |
| -------------------- | ---------------------------------------------------------------------------------- |
| `--dag FILE`         | DAG JSON file (`-` = stdin). Required.                                              |
| `-j, --jobs N`       | Max concurrent steps. Default: the machine's CPU count.                            |
| `-k, --keep-going`   | Run every still-runnable step even after a failure; report all failures at the end. |
| `--cgroups`          | Best-effort Linux cgroup-v2 per-step boxing (see [Status](#status--limitations)).  |
| `-v`                 | Verbose: stream each step's child output live as it runs.                          |
| `-q, --quiet`        | Quieter: suppress the per-step PASS summaries (failures are always shown).         |

By default (`--keep-going` off), the **first genuine failure stops scheduling** and any in-flight
steps are cancelled and reported as `ABORTED`; steps whose dependencies failed are reported as
`skipped`. Example:

```
$ safe-ci-dag-runner run --dag dag.json -j 4
...
safe-ci-dag-runner: FAIL - 1 passed, 1 failed, 1 aborted, 1 skipped in 0.2s
```

Set `NO_COLOR=1` (or pipe to a non-terminal) to disable ANSI colors.

Render a picture of the graph:

```sh
safe-ci-dag-runner dot --dag dag.json | dot -Tsvg -o dag.svg
```

## Python API

The same engine is available as a library:

```python
from safe_ci_dag_runner import Step, ResourceHint, DagConfig, run_dag, to_ascii

cfg = DagConfig(
    steps=(
        Step("build", "app", "compile", "make build",
             hint=ResourceHint(est_duration_s=90, rss_baseline_bytes=2 * 1024**3)),
        Step("test", "unit", "unit tests", "make test", deps=["build.app"]),
        Step("e2e", "smoke", "browser smoke", "make e2e", deps=["build.app"],
             hint=ResourceHint(resources={"browser": 1})),
    ),
    resource_caps={"browser": 1},   # at most one browser step at a time
)

print(to_ascii(cfg))                # visualize
result = run_dag(cfg, jobs=4)       # returns a RunResult
print(result.ok)                    # overall pass/fail (bool)
for outcome in result.outcomes:
    print(outcome.tag, outcome.ok, outcome.returncode, outcome.reason)
```

`run_dag` returns a `RunResult` (`.ok`, `.wall_s`, `.outcomes`, `.skipped`, `.step_profile_rows`);
each `StepOutcome` carries `.tag`, `.ok`, `.duration_s`, `.summary`, `.returncode`, `.reason`, and
`.aborted`. See [`USER_GUIDE.md`](USER_GUIDE.md) for the full API, the complete JSON schema, and how
to plug in a real cgroup manager or a metrics sink.

## Exit codes

| Code | Meaning                                                  |
| ---- | ------------------------------------------------------- |
| `0`  | Every step passed.                                      |
| `1`  | A step failed (or was aborted/skipped because one did). |
| `2`  | Bad usage, or a missing / malformed DAG file.           |

## Status & limitations

Stated honestly for the 0.1 release:

- **Python CLI + API: ready.** `run`, `list`, `ascii`, `dot`, `json`, and `quickstart` all work, and
  the scheduler (dependency gating, resource caps, longest-first dispatch, fail-fast / keep-going,
  failure classification) is complete.
- **cgroup boxing is best-effort, and off by default.** The safe default is `NoopCgroups` (no
  boxing; teardown falls back to a process-group kill). Real two-level cgroup-v2 boxing needs a Linux
  host with a *delegated* cgroup-v2 hierarchy. From the CLI, `--cgroups` enables per-step boxing only
  when the runner is **already inside** such a delegated scope; otherwise it prints a visible
  "containment is DEGRADED" warning and runs the steps un-boxed (it never silently drops a cap). The
  full outer-scope re-exec that sets up the two-level box is available through the Python API
  (`safe_ci_dag_runner.cgroup.reexec_in_scope` + `Cgroups`); wiring it automatically into the CLI is
  still to come.
- **Memory-aware `-j` is API-only for now.** The CLI runs at `-j`/CPU-count. The memory model and the
  "largest `-jN` that fits a RAM budget" solver (`jobs_for_budget`, `schedulable_peak_mem_bytes`) are
  available in the Python API and can be used to choose a RAM-safe `-j` yourself; per-step inner
  memory caps *are* applied automatically when cgroup boxing is active.
- **Per-step CSV metrics are baseline / experimental.** A custom `MetricsSink` works today (the
  default is a no-op that records nothing). The bundled file-backed `CsvMetricsSink` does not yet
  accept the scheduler's per-step row schema, so prefer `metrics=None` (the default) or your own sink
  for the 0.1 release. See the troubleshooting section of the user guide.
- **The Rust crate is a stub.** It currently answers only `--version`/`--help`; the runner is being
  ported and kept behaviorally identical to the Python build via differential tests. Use the Python
  package for real work.

## See also

- [`USER_GUIDE.md`](USER_GUIDE.md) — concepts, the complete DAG JSON schema, worked examples, the
  in-depth Python API, and troubleshooting.
- `safe-ci-dag-runner quickstart` — the same tour from the command line.

## License

MIT.
