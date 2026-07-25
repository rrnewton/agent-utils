# safe-ci-dag-runner — User Guide

This guide goes deeper than the [README](README.md): the core concepts, the complete DAG JSON
schema, worked examples, the full Python API, and troubleshooting. If you just want to get running,
start with the README's 60-second quickstart or run `safe-ci-dag-runner quickstart`.

## Contents

- [Concepts](#concepts)
- [The DAG JSON schema](#the-dag-json-schema)
- [Worked examples](#worked-examples)
- [The Python API in depth](#the-python-api-in-depth)
- [Troubleshooting](#troubleshooting)

## Concepts

**DAG.** Your build/test pipeline is a directed acyclic graph: a set of steps, where an edge means
"this step depends on that one." The runner executes steps concurrently, only starting a step once
all of its dependencies have finished successfully.

**Step.** One node in the graph: a shell command (`cmd`, run via `bash -c`) plus its dependencies and
an optional resource hint. Every step is identified by its **tag**, `group.job` (for example
`build.app`). Tags must be unique — they are how dependencies, resource accounting, and per-step
boxing all refer to a step.

**Dependencies.** A step lists the tags it depends on in `deps`. A step launches only once **every**
dependency has a *done and successful* outcome. If any dependency **fails**, the step is never run —
and the failure closes transitively, so everything downstream of a failure is marked `skipped`.

**ResourceHint.** Optional per-step scheduling knowledge attached to a step. Every field is optional;
supplying them unlocks smarter behavior:

- `resources` — how much of each *scarce named resource* the step needs (e.g. `{"browser": 1}`).
- `est_duration_s` — an estimated runtime, used only to *order* ready steps.
- `rss_baseline_bytes` / `hard_mem_max_bytes` — memory estimates that feed the memory model and the
  per-step inner memory cap.
- `classification` — `cpu-bound`, `latency-bound`, or `light`.
- `preferred_inner_jobs` — the step's own internal parallelism width.

**resource_caps.** A top-level map bounding the total concurrent demand for each named resource. The
runner never lets the summed `resources` demand of the steps running at one instant exceed the cap.
The classic use is `{"browser": 1}` to **serialize** browser/e2e steps (only one at a time) even
while other steps run in parallel. Resource names are arbitrary strings you choose (`browser`, `db`,
`gpu`, `net`, ...); a step demands them via its hint, and the cap bounds them.

**Longest-first (LPT) scheduling.** When several steps are ready and a slot frees up, the runner
picks the one with the largest `est_duration_s` first (a stable sort, so steps with equal or no
estimate keep their registration order). This "longest processing time" heuristic keeps big steps off
the tail of the run, shortening overall wall-clock time. It only decides *ordering*; dependency and
resource gating are enforced independently.

**Memory-aware `-j`.** Rather than a flat "N GB per job" guess, the memory model enumerates which
steps could *actually* co-run — no transitive dependency between them, and their summed resource
demand fits the caps — and takes the worst-case sum of their per-step memory caps. That yields an
exact "largest `-jN` that fits budget M." (In 0.1 this is a Python-API helper, `jobs_for_budget`; the
CLI's default `-j` is the CPU count.)

**cgroup boxing.** On a Linux cgroup-v2 host with a *delegated* hierarchy, the runner can put the
whole run in an outer CPU/memory box and each step in its own nested box. The payoff is twofold:
(1) a step that exceeds its memory cap is OOM-killed in isolation instead of taking down the host;
(2) teardown writes the step's `cgroup.kill`, an **atomic SIGKILL of the entire subtree** that
catches `setsid`/double-fork escapees (orphan servers, browsers) a process-group kill misses. When a
delegated cgroup is unavailable, the runner degrades to a process-group kill and says so out loud.

**Metrics sink.** An optional pluggable destination for durable measurements: one whole-run summary
row (wall time, CPU contention split, cores) and per-step rows (CPU, memory peak, threads, ambient
load bucket). The default records nothing. Metrics never fail a run.

**Visualization.** `to_dot` renders Graphviz DOT (one cluster per group, solid dependency edges, and
a dashed chain across the users of each cap-1 resource to hint that they serialize). `to_ascii`
renders a compact topological-layer view for the terminal.

## The DAG JSON schema

A DAG file is a single JSON object. Parsing is **strict**: a wrong-typed or malformed field is a hard
error (exit `2`), never a silent default. Only `steps`, and within each step `group`/`job`/`cmd`, are
required; everything else defaults. `safe-ci-dag-runner json --dag FILE` re-emits any DAG with every
field filled in, which is the easiest way to see the defaults applied to your file.

### Top-level fields

| Field                    | Type                | Default            | Meaning                                                                                                   |
| ------------------------ | ------------------- | ------------------ | --------------------------------------------------------------------------------------------------------- |
| `steps`                  | array of step       | — (required)       | The graph's nodes. May be empty (`[]`), which is a valid, no-op DAG.                                       |
| `resource_caps`          | object `{str: int}` | `{}`               | Capacity per named scarce resource. Concurrent summed `hint.resources` demand never exceeds this.         |
| `mem_cap_factor`         | number              | `1.25`             | Multiplier from a step's `rss_baseline_bytes` to its derived inner memory cap (headroom).                  |
| `mem_cap_floor_bytes`    | integer             | `8589934592` (8 GiB) | Lower bound on the modeled worst-case footprint, so `-j` selection never concludes "0 fits."             |
| `outer_mem_safety_factor`| number              | `1.0`              | Multiplier applied to the modeled peak footprint (`1.0` = no inflation).                                   |
| `default_step_timeout`   | integer             | `1800`             | Default per-step timeout (seconds) carried as caller policy. Note: the runner enforces each step's own `timeout`. |

### Step fields

| Field         | Type                | Default        | Meaning                                                                                                      |
| ------------- | ------------------- | -------------- | ----------------------------------------------------------------------------------------------------------- |
| `group`       | string              | — (required)   | First half of the tag.                                                                                       |
| `job`         | string              | — (required)   | Second half of the tag. The full tag `group.job` must be unique across the DAG.                             |
| `cmd`         | string              | — (required)   | Shell command, run via `bash -c` from the current working directory, in its own session/process group.      |
| `desc`        | string              | `""`           | Human-readable description shown by `list` and `run`.                                                        |
| `deps`        | array of string     | `[]`           | Tags this step depends on. All must finish successfully before it launches.                                  |
| `env`         | object `{str: str}` | `{}`           | Extra environment variables, merged over the runner's environment for this step's command.                  |
| `hint`        | object              | `{}`           | The resource hint (see below).                                                                               |
| `networkonly` | boolean             | `false`        | Caller-facing selection flag (a preset can drop these when networking is disabled). Preserved but not acted on by the core scheduler. |
| `engine_only` | boolean             | `false`        | Caller-facing subset flag. Concretely, `engine_only` steps are **excluded from the memory-budget model**.    |
| `timeout`     | integer             | `1800`         | Per-step timeout in seconds. On expiry the step's whole process tree is reaped and the step FAILS as TIMEOUT. |

### Hint (`hint`) fields

| Field                      | Type                | Default    | Meaning                                                                                                       |
| -------------------------- | ------------------- | ---------- | ------------------------------------------------------------------------------------------------------------ |
| `resources`                | object `{str: int}` | `{}`       | Scarce-resource demand for this step, matched against `resource_caps`.                                        |
| `est_duration_s`           | number              | `0.0`      | Estimated wall-clock seconds; orders ready steps (longest first). `0` sorts last. Not a correctness contract. |
| `rss_baseline_bytes`       | integer or `null`   | `null`     | Estimated peak resident memory. Enables the memory model and (with `mem_cap_factor`) the derived inner cap. `null` excludes the step from the memory model. |
| `hard_mem_max_bytes`       | integer or `null`   | `null`     | Explicit hard inner memory cap (bytes); overrides the derived (`rss_baseline_bytes` × `mem_cap_factor`) cap.  |
| `classification`           | `"cpu-bound"` / `"latency-bound"` / `"light"` | `"light"` | How the step uses the machine. Any browser-resource step is treated as `latency-bound` automatically. |
| `preferred_inner_jobs`     | integer or `null`   | `null`     | The step's own internal parallelism width. Sets the inner CPU cap when boxed and scales the memory-budget model. It does **not** inject a `-j` flag into your command — your `cmd` must use its own parallelism. |
| `measured_effective_cores` | number or `null`    | `null`     | Measurement passthrough (recorded in metrics; not used for scheduling).                                       |
| `measured_cpu_utilization` | number or `null`    | `null`     | Measurement passthrough (recorded in metrics; not used for scheduling).                                       |

## Worked examples

### A diamond

`build.app` fans out to two independent steps, which both feed `deploy.prod`:

```json
{
  "steps": [
    {"group": "build", "job": "app", "desc": "compile", "cmd": "make build",
     "hint": {"est_duration_s": 90, "classification": "cpu-bound"}},
    {"group": "test", "job": "unit", "desc": "unit tests", "cmd": "make test",
     "deps": ["build.app"], "hint": {"est_duration_s": 120}},
    {"group": "test", "job": "lint", "desc": "lint", "cmd": "make lint",
     "deps": ["build.app"], "hint": {"est_duration_s": 20}},
    {"group": "deploy", "job": "prod", "desc": "deploy", "cmd": "make deploy",
     "deps": ["test.unit", "test.lint"]}
  ]
}
```

`test.unit` and `test.lint` run concurrently after `build.app`; `deploy.prod` waits for both. Because
`test.unit` has the larger `est_duration_s`, it is dispatched first when the two become ready. If
`test.lint` fails, `deploy.prod` is `skipped` and (by default) any in-flight `test.unit` is
`ABORTED`.

### Serializing browser steps with a resource cap

Three end-to-end steps that each need "the browser" — but you only want one running at a time so they
don't fight over ports or a display, while unit tests still run in parallel:

```json
{
  "resource_caps": {"browser": 1},
  "steps": [
    {"group": "build", "job": "app", "cmd": "make build"},
    {"group": "e2e", "job": "login",  "cmd": "make e2e-login",  "deps": ["build.app"],
     "hint": {"resources": {"browser": 1}}},
    {"group": "e2e", "job": "search", "cmd": "make e2e-search", "deps": ["build.app"],
     "hint": {"resources": {"browser": 1}}},
    {"group": "e2e", "job": "cart",   "cmd": "make e2e-cart",   "deps": ["build.app"],
     "hint": {"resources": {"browser": 1}}},
    {"group": "test", "job": "unit",  "cmd": "make test",       "deps": ["build.app"]}
  ]
}
```

All three `e2e.*` steps are ready at once, but the `browser: 1` cap admits only one at a time; the
other two wait for the resource to free, while `test.unit` runs alongside whichever e2e step holds
the browser. (`safe-ci-dag-runner dot` draws a dashed edge across the browser users to show they
serialize.)

### A step with internal parallelism

A build that runs its own `-j` internally. Declare the width so the runner can set an inner CPU cap
(when boxed) and account for the extra memory in the budget model:

```json
{
  "steps": [
    {"group": "build", "job": "app", "desc": "parallel compile",
     "cmd": "make -j8 build",
     "hint": {"classification": "cpu-bound", "preferred_inner_jobs": 8,
              "rss_baseline_bytes": 6442450944}}
  ]
}
```

The `-j8` is in *your* command — `preferred_inner_jobs: 8` only tells the runner about it. When cgroup
boxing is active the step is capped to 8 CPUs; in the memory model a `cpu-bound` step's cap scales
with its inner width above a width of 4.

## The Python API in depth

Everything the CLI does is importable from the top-level package.

```python
from safe_ci_dag_runner import (
    Step, ResourceHint, DagConfig, StepClass,   # model
    run_dag, RunResult, StepOutcome,            # running
    to_ascii, to_dot,                           # visualization
    dag_from_json, dag_to_json, DagJsonError,   # (de)serialization
)
```

### `run_dag`

```python
run_dag(
    cfg: DagConfig,
    *,
    jobs: int,                       # outer fan-out (-j); clamped to >= 1
    cgroups: CgroupManager | None = None,   # None -> NoopCgroups (no boxing)
    metrics: MetricsSink | None = None,      # None -> record nothing
    keep_going: bool = False,        # run all runnable steps even after a failure
    verbosity: int = 1,              # 0 quiet(+failures), 1 default(+summaries), >=2 stream child stdout
) -> RunResult
```

`jobs` is required and keyword-only. Passing a `cgroups` manager whose `.enabled` is `False` triggers
a visible "containment is DEGRADED" warning (No Silent Failure) and runs un-boxed. Passing a real
`metrics` sink but getting nothing recorded also warns.

### `RunResult` and `StepOutcome`

`run_dag` returns a frozen `RunResult`:

| Field               | Type                        | Meaning                                                              |
| ------------------- | --------------------------- | ------------------------------------------------------------------- |
| `ok`                | `bool`                      | Overall pass/fail — `True` iff no genuine failure occurred.         |
| `wall_s`            | `float`                     | Wall-clock seconds for the whole run.                               |
| `outcomes`          | `tuple[StepOutcome, ...]`   | Per-step results (only steps that actually ran), in dispatch order. |
| `skipped`           | `tuple[str, ...]`           | Sorted tags whose dependencies failed, so they never ran.           |
| `step_profile_rows` | `tuple[Mapping, ...]`       | Per-step measurement rows, to forward to a `MetricsSink`.           |

Each `StepOutcome` is frozen too:

| Field         | Type            | Meaning                                                                              |
| ------------- | --------------- | ----------------------------------------------------------------------------------- |
| `tag`         | `str`           | The step's `group.job`.                                                              |
| `ok`          | `bool`          | Whether the step succeeded (exit 0, not timed out, not aborted).                     |
| `duration_s`  | `float`         | Wall-clock seconds the step ran.                                                     |
| `summary`     | `str`           | One-line summary (the last non-empty line of the step's output), `""` if none.      |
| `returncode`  | `int` or `None` | Child exit code; negative for a Unix signal; `None` if never collected.             |
| `reason`      | `str`           | Human-readable failure reason (e.g. `exit 3`, `TIMEOUT >1800s`, `OOM-KILLED ...`); `""` when `ok`. |
| `aborted`     | `bool`          | `True` when eager-exit cancelled this in-flight step after *another* step failed.    |

Failure reasons follow a fixed precedence: OOM > timeout > pids-guard > detail-capture > signal >
exit code — so an externally-signalled kill is never misreported as an out-of-memory.

### Building a DAG from / to JSON

```python
cfg = dag_from_json(open("dag.json").read())   # raises DagJsonError on a bad document
print(dag_to_json(cfg))                          # canonical, fully-defaulted, 2-space JSON
```

### Visualization

```python
to_ascii(cfg, selected=None)              # -> str  (topological-layer ASCII)
to_dot(cfg, name="dag", selected=None)    # -> str  (Graphviz DOT)
```

Pass `selected={"build.app", "test.unit"}` to render only a subset of tags (dependencies to steps
outside the subset are dropped from the drawing).

### Choosing a RAM-safe `-j` (memory-aware sizing)

The CLI defaults `-j` to the CPU count, but you can size it to real memory in the API:

```python
from safe_ci_dag_runner import jobs_for_budget, mem_available_bytes, run_dag

budget = mem_available_bytes() or 8 * 1024**3   # bytes; falls back if /proc unreadable
jobs, footprint = jobs_for_budget(cfg, budget)  # largest -jN (>=1, capped at CPU count) that fits
result = run_dag(cfg, jobs=jobs)
```

Related helpers: `schedulable_peak_mem_bytes(cfg, jobs)` (worst-case co-running memory and which
steps), `jobs_footprint_bytes(cfg, jobs)`, `step_mem_cap_bytes(step, mem_cap_factor=...)`,
`transitive_deps(steps)`, and `parse_size("8G")` (parse `8G`/`4096M`/`2048K`/bytes → int). Only steps
that carry an `rss_baseline_bytes` and are not `engine_only` participate in the memory model.

### Plugging in a metrics sink

A `MetricsSink` has two methods; supply your own to capture measurements. This minimal in-memory sink
works today:

```python
from safe_ci_dag_runner import run_dag, Step, DagConfig
from safe_ci_dag_runner.protocols import MetricsSink, RunWindow

class MyWindow:
    def finish(self, *, result, n_steps, jobs):
        print(f"run {result}: {n_steps} steps at -j{jobs}")
        return {"result": result}

class MySink(MetricsSink):
    def start_run_window(self) -> RunWindow:
        return MyWindow()
    def record_step_profiles(self, rows, *, jobs):
        for row in rows:               # row: step, classification, elapsed_s, returncode,
            print(row["step"], row["ok"], row.get("peak_bytes"))   # ok, oom_kills, peak_bytes, ...
        return "in-memory"             # a location string, or None if you skipped recording

run_dag(DagConfig(steps=(Step("build", "app", "compile", "echo hi"),)), jobs=2, metrics=MySink())
```

A file-backed `CsvMetricsSink` also ships, but it is baseline/experimental in 0.1 — see
troubleshooting below. The safe default is `metrics=None` (record nothing).

### Plugging in cgroup containment

`run_dag(cgroups=...)` accepts anything satisfying the `CgroupManager` protocol. Two are bundled:

- `NoopCgroups` — the safe default (`enabled == False`): no boxing, teardown falls back to a
  process-group kill. Works on any host.
- `Cgroups` — the real Linux cgroup-v2 manager. It is `enabled` **only** when the process is already
  inside a delegated cgroup-v2 scope. To set that scope up, use the API in
  `safe_ci_dag_runner.cgroup`:

  ```python
  import sys
  from safe_ci_dag_runner.cgroup import reexec_in_scope, Cgroups
  from safe_ci_dag_runner import run_dag

  # Re-exec this process inside a transient, delegated systemd --user scope. On success this
  # replaces the process and does not return; the re-executed run then boxes each step.
  if not reexec_in_scope(sys.argv, memory_max=None):
      sys.exit("refusing to run without cgroup enforcement")
  cg = Cgroups()          # enabled==True now that we're in the delegated scope
  run_dag(cfg, jobs=4, cgroups=cg)
  ```

  When boxing is active, each step's inner memory cap comes from its hint (`hard_mem_max_bytes`, else
  `rss_baseline_bytes` × `mem_cap_factor`) and its inner CPU cap from `preferred_inner_jobs`.

## Troubleshooting

**"containment is DEGRADED" / steps run un-boxed.** You passed `--cgroups` (or a `Cgroups()` whose
`.enabled` is `False`). The runner prints, then continues un-boxed:

```
[scheduler] ⚠ per-step cgroup manager is present but disabled; containment is DEGRADED (falling back to process-group kill for teardown, no inner memory/CPU caps).
```

This is expected when the process is **not** inside a delegated cgroup-v2 scope (a non-Linux host, no
systemd `--user` delegation, or you didn't set up the outer scope). Steps still run correctly;
teardown just uses a process-group kill instead of `cgroup.kill`, and no inner memory/CPU caps are
applied. To get real boxing, run inside a delegated scope via `reexec_in_scope` (see above). The
warning is deliberate — the tool never silently drops a cap you asked for.

**A DAG file is rejected (exit 2).** Parsing is strict and the message names the offending field:

```
$ safe-ci-dag-runner run --dag bad.json
safe-ci-dag-runner: steps[0]: field 'cmd' must be a string           # missing/wrong-typed required field
safe-ci-dag-runner: invalid JSON: Expecting value: line 1 column 1   # not JSON at all
safe-ci-dag-runner: steps[0].hint.classification: unknown value 'gpu'# not cpu-bound/latency-bound/light
safe-ci-dag-runner: [Errno 2] No such file or directory: 'bad.json'  # file missing
```

Fix the named field. `classification` must be exactly `cpu-bound`, `latency-bound`, or `light`; sizes
must be integers (bytes) — the JSON schema does not accept `"8G"` strings (that shorthand is only in
the `parse_size` API helper).

**Colors are noisy in logs / CI.** Set `NO_COLOR=1` to disable ANSI colors; they are also
auto-disabled whenever stdout is not a terminal.

**Where does output go?** Per-step `▶ START` / `✓ PASS` / `✗ FAIL` / `⊘ ABORT` lines and the failing
step's captured detail go to **stdout**; the final `PASS`/`FAIL` summary line goes to **stderr**. So
`run ... 2>/dev/null` hides only the summary line.

**`CsvMetricsSink` raises `ValueError: dict contains fields not in fieldnames`.** Known limitation in
0.1: the bundled file-backed CSV sink does not yet accept the scheduler's per-step row schema. Use
`metrics=None` (the default) or a custom `MetricsSink` (see above) until it is wired up.

**The Rust binary does nothing.** The Rust crate is a stub in 0.1 (only `--version`/`--help`). Use
the Python package (`pip install "git+https://github.com/rrnewton/agent-utils#subdirectory=py"`).
