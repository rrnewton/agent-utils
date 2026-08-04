# safe-ci-dag-runner

Run a **directed acyclic graph (DAG) of CI / build / test steps** concurrently and *safely*.
You describe your steps as shell commands with dependencies (and optional resource hints) in a small
JSON **or YAML** file (or in Python), and the runner schedules them across a worker pool —
respecting dependencies and scarce-resource limits — while boxing each step so a runaway command
cannot take down the run or the host.

## What you get

- **Two-level cgroup boxing, on by default.** `run` boxes every step: the whole run executes inside
  an outer CPU/memory box, and each step gets its own nested box. A step that blows its memory budget is OOM-killed *in isolation* at
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

Boxing is the tool's primary purpose, so `run` boxes each step **by default**. On a machine without
cgroup-v2 + a working systemd `--user` scope (many laptops, most CI runners, macOS), a bare `run`
**errors with exit code 3** instead of silently running unprotected; pass `--allow-cgroup-failure` to
fall back to the safe no-op stand-in (`NoopCgroups`) and run un-boxed with a visible warning.
Containment and metrics are **pluggable** (the `CgroupManager` and `MetricsSink` protocols), so the
scheduler runs anywhere.

This is one tool from [`agent-utils`](https://github.com/rrnewton/agent-utils). It ships as a Python
package and a Rust crate, **at full parity**: both builds load a DAG from JSON, model it,
size it (memory-aware `-j`), visualize it, run it under two-level cgroup-v2 boxing, and write per-step
perf CSVs — and a randomized differential test asserts the two produce identical observable output.
See [Status & limitations](#status--limitations).

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

Save a DAG as `dag.json`. Each step is identified by a fully-qualified `group.job` **tag** (the
`group` is just a namespace for organizing and referring to steps). A step carries a shell `cmd` and
may depend on other steps by tag. The top-level `resource_caps` (introduced before the per-step
`hint` because it bounds them) limits how much of each named scarce resource may run at once:

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

$ safe-ci-dag-runner run --dag dag.json --allow-cgroup-failure
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

**About `--allow-cgroup-failure`.** Boxing each step in its own cgroup-v2 sandbox is this tool's
primary purpose, so `run` **boxes by default** (it re-execs the run inside a transient `systemd-run
--user --scope` and caps each step). On a host without cgroup-v2 + a working systemd `--user` scope,
a bare `run` errors with exit code `3` rather than silently running unprotected; the flag opts out
and runs the steps un-boxed (with a warning) so the quickstart works on any machine. On a Linux host
with a systemd user session, drop the flag to get real per-step boxing.

Ready-to-run examples of each idea (linear chain, diamond, scarce resource, memory-aware sizing,
inner jobs) live in [`examples/`](https://github.com/rrnewton/agent-utils/tree/main/examples), in
both JSON and (for a couple) side-by-side YAML.

## The DAG format: JSON or YAML

A DAG file can be written in **JSON or YAML** — two spellings of the *same* schema (same fields,
same defaults, same strict validation, same model). `--dag FILE` auto-detects the format by
extension: `.yaml`/`.yml` load as YAML, everything else as JSON; `--dag -` reads a JSON DAG from
stdin.

YAML additionally allows **comments** and **multi-line block-scalar descriptions**, so a DAG can
document itself inline ("literate" DAGs). Every node — and the DAG as a whole — takes an optional
`description` field: free-form long-form documentation, distinct from the short `desc` label, that
never affects scheduling. The `yaml` subcommand re-emits a DAG as YAML and `json` re-emits canonical
JSON, so you can convert between them.

```yaml
# A literate YAML DAG (isomorphic to the equivalent JSON).
description: |-
  Build, then test. The test depends on the build artifact.
steps:
  - group: build
    job: app
    desc: compile                 # short label shown by `list`/`run`
    description: |-               # long-form docs (a literal block scalar)
      Compile the app once; the test depends on this artifact.
    cmd: make build
  - group: test
    job: unit
    cmd: make test
    deps: [build.app]
```

Loading is guaranteed **isomorphic** across the Python and Rust builds (a YAML file loads to a
byte-identical canonical JSON under both — enforced by the cross differential). One YAML gotcha:
quote scalars that look like booleans or numbers when you mean the string — e.g. `cmd: "true"`,
`desc: "no"` — since unquoted `true`/`no`/`yes`/`on`/`off` parse as booleans. See the
[USER_GUIDE](https://github.com/rrnewton/agent-utils/blob/main/common/docs/safe-ci-dag-runner/USER_GUIDE.md#yaml-an-isomorphic-literate-alternative-to-json)
for the full treatment.

## The resource model: memory, CPU, and named resources

The runner reasons about three *independent* kinds of resource; keeping them separate makes the
knobs easy to place:

1. **Memory (bytes / GB).** Per-step memory hints — `rss_baseline_bytes` (estimated peak resident
   memory) and `hard_mem_max_bytes` (an explicit cap) — drive **memory-aware `-j`**: `run --max-mem
   8G` picks the largest concurrency whose modeled worst-case RAM fits your budget, and (under cgroup
   boxing) caps each step so a runaway is OOM-killed in isolation. This is the knob most users reach
   for first.
2. **CPU.** Outer concurrency is `-j` (how many steps run at once); a step's own internal width is
   `preferred_inner_jobs` (e.g. it runs `make -j8`); and under cgroup boxing the whole run and each
   step get CPU caps from the delegated scope. CPU governs *how many things run*, not *which are
   allowed to co-run*.
3. **Named scarce resources (semaphores).** For constraints that memory and CPU do not capture, a
   step declares demand in `hint.resources` (e.g. `{"browser": 1}`) and the top-level `resource_caps`
   bounds the total concurrent demand (e.g. `{"browser": 1}` lets only one browser step run at a
   time).

**Named resources are just names you choose — not a built-in list.** A resource name is an arbitrary
caller-defined string; the only rule is that every resource a step demands must have a matching key
in `resource_caps` with enough capacity, or that step can never be scheduled. `"browser"` is the
worked example throughout these docs, and it is *only a name*: DeepScry uses it to serialize its
Playwright/browser end-to-end tests, which each grab fixed ports and a display and so cannot run
concurrently. You could equally cap `"gpu"`, `"db"`, or `"api-tokens"` — anything scarce.

## Planning algorithm

Three planners are available via `--planner`:

- **`greedy-lpt`** (default) — a greedy, **longest-processing-time-first** pass: whenever a worker
  slot frees, the runner starts the ready step (all dependencies done, resource demand fits the
  caps, under `-j`) with the largest `est_duration_s` first, so long steps do not pile up on the
  tail of the run.
- **`critical-path`** — critical-path-first list scheduling: it computes each step's **bottom-level**
  (the longest remaining path to a sink, weighted by `est_duration`) and, among ready steps,
  launches the one with the highest bottom-level first. This keeps the graph's longest chain moving
  and shortens the makespan when a cheap step heads a long dependency chain (a case where plain
  longest-*single*-step ordering picks the wrong step first).
- **`cpa`** (new in v0.8.0) — a **measured-curve moldable allocator**: it also chooses each step's
  inner `-j` width. Over the learned speedup curves it balances the critical path against the
  per-core work ("area"), giving more cores to critical-path steps that scale and leaving plateau
  steps narrow, subject to the machine's core budget, `--max-mem`, and each step's
  work-conservation knee — then critical-path list-schedules at those widths and holds the core
  budget at dispatch. It runs each step with the chosen `-j` and reports the allocated widths plus a
  makespan lower bound and modeled makespan. It's a heuristic (CPA — Radulescu & van Gemund 2001),
  not an optimal schedule; see
  [`PLANNER_DESIGN.md`](PLANNER_DESIGN.md) for the algorithm, its literature grounding, and our
  deviations.

The `greedy-lpt` and `critical-path` orders — and the `cpa` allocation and its resulting plan — are
**deterministic and identical across the Python and Rust builds** for the same profile + DAG (ties
broken stably, by tag).

**Learned estimates (the feedback loop).** `est_duration_s` no longer has to be a static hint you
hand-author. The runner now **feeds the recorded profile store back in at plan time** (ds-7pzdgm /
ds-afzsqf): for the current machine + container class it derives, per step,

- a robust `est_duration_s` — the **contention-discounted MEDIAN** of past `elapsed_s` (a median,
  not a mean, so one slow sample cannot drag it; discounted by whatever contention signal the store
  carries — `pct_other` / `external_cores` / a CPU-pressure column / `co_tenants` — to recover the
  step's *intrinsic*, uncontended duration), and
- a robust `rss_estimate_bytes` — a **high percentile** of past `peak_bytes`, for the memory model.

These **override the DAG-authored hint** once the store has enough samples (the hint is the
fallback), so planning improves automatically as runs accumulate, and the same learned `rss` feeds
the memory-aware `--max-mem` sizing. Pass `--no-profile-feedback` to ignore the store and plan from
the DAG hints only (for reproducibility). Inspect exactly what the planner decided — and why — with
the [`plan` command](#the-plan-command-see-the-estimates--the-schedule) or `run --show-plan`.

**Parallel-speedup model.** The runner also learns each step's **speedup curve** — `wall` vs. inner
`-j` — from the samples the store holds across *different* `inner_jobs` widths (the widths a `sweep`,
or repeated runs at different `preferred_inner_jobs`, records). For every step with at least two
measured widths it derives a robust, contention-discounted `wall_s` per width, the achieved
`effective_cores`, and the **work-conservation signal** (total CPU-seconds `user_s`+`sys_s`, and
`throttled_s`), then exposes the `speedup(inner_jobs)` curve, the `measured_effective_cores`, and a
**recommended `inner_jobs`** — the best wall time before the knee (a level must be ≥1.15x faster than
the previous AND not blow total CPU-seconds past 1.5x) and within the machine's core budget. So a
step whose wall halves `-j1`→`-j2` but barely improves `-j2`→`-j4` while its total CPU-seconds rise
gets a recommended `inner_jobs` of 2; a near-linear step gets the widest measured width. This is
surfaced by `plan` / `run --show-plan` (below) and is byte-identical across the two builds.

This model now has a consumer: the **`--planner cpa`** allocator (above) *acts* on these curves,
distributing inner `-j` across the DAG to minimize whole-DAG wall (more threads to critical-path
steps that scale, fewer to steps that plateau) under the total core + memory + resource caps. The
per-step `recommended_inner_jobs` here is the work-conservation knee the allocator never widens past;
the whole-DAG allocation is CPA's job. See [`PLANNER_DESIGN.md`](PLANNER_DESIGN.md).

### The `plan` command: see the estimates + the schedule

`plan` prints, without running anything, exactly what the planner would do — so a CI-optimizing agent
(or a person) can see *why* the steps are ordered the way they are:

```
$ safe-ci-dag-runner plan --dag dag.json --planner critical-path
plan: critical-path
per-step estimates (source: store = learned from the profile store; hint = DAG-authored; default = none):
step       est_duration_s  source  rss_estimate  rss_source  bottom_level_s  samples
---------  --------------  ------  ------------  ----------  --------------  -------
build.app           3.000   store       1.1 GiB       store          13.000        4
test.unit          10.000    hint             -        none          10.000        0
lint.all            5.000    hint             -        none           5.000        0
critical path (13.000s): build.app -> test.unit
scheduled order: build.app, test.unit, lint.all
```

The **source** column shows where each estimate came from (`store` = learned, `hint` = DAG-authored,
`default` = none). When the store holds more than one `inner_jobs` width for a step, `plan` /
`run --show-plan` append a **parallel-speedup section** with that step's recommended `inner_jobs`,
achieved `eff_cores`, the speedup at that width, and the full `inner_jobs->speedup` curve:

```
parallel-speedup model (recommended inner_jobs = best wall within the knee + core budget; speedup@rec = speedup at that width):
step       rec_inner_jobs  eff_cores  speedup@rec  curve(inner_jobs->speedup)
---------  --------------  ---------  -----------  --------------------------
build.app               2      1.980        2.00x     1:1.00x 2:2.00x 4:2.22x
```

With `--planner cpa` the table gains an `alloc_inner_jobs` column and a one-line allocator summary
(stop reason, core budget `P`, and the balancing terms — critical path vs. per-core area, the
makespan lower bound, and the achieved modeled makespan):

```
$ safe-ci-dag-runner plan --dag dag.json --planner cpa
...
allocator (cpa): knee-exhausted; P=16 cores; critical-path=11.000s, area/P=4.188s, lower-bound=11.000s, modeled-makespan=11.000s
```

`plan --format json` emits the same plan as canonical, machine-readable JSON (byte-identical across
the Python and Rust builds), including a per-step `"speedup"` object (or `null`), a per-step
`"alloc_inner_jobs"`, and a top-level `"allocation"` object (`null` unless `--planner cpa`).
`run --show-plan` prints this table and then runs. All planners honor `--planner` and
`--no-profile-feedback`; `--planner cpa` also honors `--max-mem`.

## CLI reference

```
safe-ci-dag-runner <command> [options]
```

Every command except `quickstart` takes `--dag FILE`. The format is **auto-detected by extension**:
`.yaml`/`.yml` load as YAML, everything else as JSON (use `-` to read a JSON DAG from stdin). YAML is
isomorphic to the JSON schema — see [The DAG format: JSON or YAML](#the-dag-format-json-or-yaml).

| Command      | What it does                                                        |
| ------------ | ------------------------------------------------------------------- |
| `run`        | Run the DAG. Exit `0` iff every step passes.                        |
| `sweep`      | Parallel-speedup study of ONE step: run it at inner `-j1..-jN` and print a timing/speedup table (see [Profiling & experimenting with individual steps](#profiling--experimenting-with-individual-steps)). |
| `plan`       | Show the plan — per-step learned/hint estimates, the critical path, and the scheduled order — without running anything (see [The `plan` command](#the-plan-command-see-the-estimates--the-schedule)). |
| `list`       | List the steps with class and dependencies (registration order).    |
| `ascii`      | Draw the DAG as ASCII art, grouped by topological layer.            |
| `dot`        | Emit Graphviz DOT to stdout (pipe to `dot -Tsvg`).                  |
| `json`       | Re-emit the DAG as canonical, fully-defaulted JSON.                 |
| `yaml`       | Re-emit the DAG as YAML.                                            |
| `quickstart` | Print a self-contained getting-started guide (no `--dag` needed).  |
| `--userguide`| Print this full user guide (the complete reference), embedded in the tool so it works after `pip install` / `cargo install`. Byte-identical across the Python and Rust builds. |

Global: `--version`, `-h/--help`. Running with no command prints help and exits `0`.

### `run` flags

| Flag                 | Meaning                                                                             |
| -------------------- | ---------------------------------------------------------------------------------- |
| `--dag FILE`         | DAG JSON file (`-` = stdin). Required.                                              |
| `-j, --jobs N`       | Max concurrent steps. Default: the machine's CPU count.                            |
| `--only TAG[,TAG...]`| Run **exactly** the named step(s) and nothing else. Dependency edges to steps *outside* the selection are dropped (their outputs are assumed already present) — it does **not** run the step's dependencies. For profiling/experimenting on a step in isolation. Errors (exit `2`) if a tag does not exist. See [Profiling & experimenting with individual steps](#profiling--experimenting-with-individual-steps). |
| `--max-mem SPEC`     | RAM budget (e.g. `8G`, `4096M`): pick the largest `-j` whose modeled worst-case footprint fits. Ignored when `--jobs` is given (`--jobs` wins, with a note). |
| `--planner NAME`     | Dispatch-ordering planner: `greedy-lpt` (default; longest single step first) or `critical-path` (longest remaining est-weighted path first). See [Planning algorithm](#planning-algorithm). |
| `--show-plan`        | Before running, print the plan (per-step estimate + source, `rss_estimate`, bottom-level, the critical path, and the scheduled order). |
| `--no-profile-feedback` | Do **not** read the profile store to refine `est_duration_s` / `rss_baseline_bytes` at plan time; plan from the DAG hints only (for reproducibility). |
| `--profile`          | After the run, print a per-step profile table (`step` &#124; `wall_s` &#124; `user_s` &#124; `sys_s` &#124; `rss_hwm` &#124; `oom` &#124; `inner_jobs`) to the terminal. The CPU/memory columns come from the per-step cgroup, so they are populated under boxing and show `-` in an un-boxed run. |
| `--perf-dir DIR`     | Write per-step and whole-run resource-usage CSVs into `DIR` (uses the `CsvMetricsSink`), **overriding** the default profile store and `$SAFE_CI_DAG_RUNNER_PROFILE_DIR`. Prints the CSV paths at the end. |
| `--no-profile`       | Disable the default auto-logging profile store (do not append CSVs anywhere). |
| `-k, --keep-going`   | On a failure, let already-running steps finish instead of eager-cancelling them (still stops launching new steps). |
| `--allow-cgroup-failure` | Opt out of the on-by-default boxing requirement: instead of erroring (exit `3`) when cgroup-v2 + a systemd `--user` scope are unavailable, run the steps **un-boxed** with a visible warning. Needed on laptops, most CI runners, and non-Linux hosts. |
| `-v`                 | Verbose: stream each step's child output live as it runs.                          |
| `-q, --quiet`        | Quieter: suppress the per-step PASS summaries (failures are always shown).         |

### Profiling & experimenting with individual steps

Beyond running a whole DAG, three tools let an agent (or a person) profile and experiment with
**individual** steps, and make profiling data land somewhere obvious so it can be browsed:

**`run --only TAG[,TAG...]`** runs *exactly* the named step(s) and nothing else. It does **not** run
their dependencies — dependency edges to steps outside the selection are dropped, and the step's
inputs are assumed already present — so you can iterate on one step without re-running the graph:

```sh
safe-ci-dag-runner run --dag dag.json --only build.app --allow-cgroup-failure
safe-ci-dag-runner run --dag dag.json --only build.app,test.unit --allow-cgroup-failure
```

**`run --profile`** prints a per-step profile table after the run so a CI-optimizing agent sees what
happened without opening a CSV:

```
$ safe-ci-dag-runner run --dag dag.json --profile
...
per-step profile:
step       wall_s  user_s  sys_s  rss_hwm  oom  inner_jobs
---------  ------  ------  -----  -------  ---  ----------
build.app   0.920   3.438  0.004  2.6 MiB    0           4
test.unit   0.269   0.002  0.016  512.0 KiB  0     ambient
```

**`sweep`** runs ONE step at inner parallelism `-j1, -j2, … -jN` (passing each width into the step's
command via its `jobs_flag`) and prints the classic parallel-speedup table:

```
$ safe-ci-dag-runner sweep --dag dag.json --step build.app --jobs 1..8
parallel-speedup sweep: build.app
jobs  wall_s  user_s  sys_s  rss_hwm  speedup(vs j1)
----  ------  ------  -----  -------  --------------
1      3.574   3.514  0.007  1.2 MiB           1.00x
2      1.770   3.432  0.003  1.6 MiB           2.02x
4      1.457   3.487  0.006  2.6 MiB           2.45x
8      0.519   3.553  0.020  3.7 MiB           6.89x
```

`sweep` flags: `--dag FILE`, `--step TAG` (the single `group.job` step to sweep), `--jobs RANGE`
(`LO..HI`, e.g. `1..8`, or a bare `N` meaning `1..N`), `--repeat K` (run each width K times and keep
the fastest wall time; default `1`), plus `--perf-dir` / `--no-profile` / `--allow-cgroup-failure` /
`-v` with the same meaning as `run`. Like `run`, `sweep` boxes each measurement by default so it
measures under real cgroup limits (`rss_hwm` comes from the step cgroup's `memory.peak`, so it is
populated under boxing and blank in an un-boxed `--allow-cgroup-failure` run).

### Where profiling data is stored (the default profile store)

Every `run` and `sweep` **auto-logs** resource-usage CSVs — you do **not** need `--perf-dir`. The
destination is resolved in this order:

1. `--no-profile` — logging is disabled.
2. `--perf-dir DIR` — an explicit directory (wins over everything below).
3. `$SAFE_CI_DAG_RUNNER_PROFILE_DIR` — an environment override.
4. **Default:** `./.safe-ci-dag-runner/profiles/` — repo-local, relative to the current working
   directory, created on demand.

The tool prints exactly where it appended (never a silent write), e.g.:

```
safe-ci-dag-runner: profile data appended to the default profile store at .safe-ci-dag-runner/profiles
  (override with --perf-dir or $SAFE_CI_DAG_RUNNER_PROFILE_DIR; disable with --no-profile):
  .safe-ci-dag-runner/profiles/<machine>.csv
  .safe-ci-dag-runner/profiles/step_profiles_<machine>_<container>.csv
```

The schema and filenames are the same as `--perf-dir` has always produced: a whole-run summary
`<machine_id>.csv` and a per-step `step_profiles_<machine>_<container>.csv` (one row per step, with
dynamic cgroup `cpu.*` columns when boxing is active — including the `inner_jobs` width, so a sweep's
rows are browsable per width). Consider **gitignoring** `./.safe-ci-dag-runner/` (it is machine-local
perf data, not source) — or check it in to keep a history; that is a project choice.

Memory-aware sizing example — let the runner choose a RAM-safe `-j` from the graph's per-step
memory hints instead of hard-coding one:

```sh
safe-ci-dag-runner run --dag dag.json --max-mem 8G --allow-cgroup-failure
```

(As everywhere, drop `--allow-cgroup-failure` on a Linux host with a systemd user session to get
real per-step boxing; keep it where cgroups are unavailable.) `--max-mem` only throttles a DAG whose steps carry per-step `rss_baseline_bytes` memory hints. With
no baselines, the modeled footprint collapses to `mem_cap_floor_bytes` (default 8 GiB), so any budget
at or above the floor picks the full `-j` — the **CPU count** — and prints a note saying it did not
throttle. "CPU count" here means **logical** CPUs (`os.cpu_count()`, i.e. SMT/hyperthreads included).

Record resource usage while running:

```sh
safe-ci-dag-runner run --dag dag.json --perf-dir ./perf --allow-cgroup-failure
# writes ./perf/step_profiles_<machine>_<container>.csv (one row per step)
#    and ./perf/<machine>.csv                            (one whole-run summary row)
```

By default (`--keep-going` off), the **first genuine failure stops scheduling** and any in-flight
steps are cancelled and reported as `ABORTED`; steps whose dependencies failed are reported as
`skipped`. Example:

```
$ safe-ci-dag-runner run --dag dag.json -j 4 --allow-cgroup-failure
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
        Step("build", "app", "compile", "echo building && sleep 0.1",
             hint=ResourceHint(est_duration_s=90, rss_baseline_bytes=2 * 1024**3)),
        Step("test", "unit", "unit tests", "echo unit && sleep 0.1", deps=["build.app"]),
        Step("e2e", "smoke", "browser smoke", "echo e2e && sleep 0.1", deps=["build.app"],
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
`.aborted`. See [`USER_GUIDE.md`](https://github.com/rrnewton/agent-utils/blob/main/common/docs/safe-ci-dag-runner/USER_GUIDE.md) for the full API, the complete JSON schema, and how
to plug in a real cgroup manager or a metrics sink.

## Exit codes

| Code | Meaning                                                  |
| ---- | ------------------------------------------------------- |
| `0`  | Every step passed.                                      |
| `1`  | A step failed (or was aborted/skipped because one did). |
| `2`  | Bad usage, or a missing / malformed DAG file.           |
| `3`  | cgroup boxing was required (the default) but could not be established; re-run with `--allow-cgroup-failure` to run un-boxed. |

## Status & limitations

Stated honestly:

- **Python CLI + API: ready.** `run`, `sweep`, `plan`, `list`, `ascii`, `dot`, `json`, `yaml`, and
  `quickstart` all work, and the scheduler (dependency gating, resource caps, `--planner` dispatch
  ordering, fail-fast / keep-going, failure classification) is complete.
- **Learned-estimate feedback loop: ready (both builds).** The runner reads the profile store back at
  plan time to derive a contention-discounted median `est_duration_s` and a high-percentile
  `rss_estimate_bytes` per step, overriding the DAG hints when enough samples exist; `--planner
  critical-path` adds bottom-level list scheduling alongside the default `greedy-lpt`; the `plan`
  command / `run --show-plan` display the estimates + schedule. Still to come: full
  parallel-speedup-curve modeling from the multi-width `sweep` samples.
- **cgroup boxing is ON by default (both builds).** `run` boxes each step: it re-execs the run inside
  a transient, delegated `systemd-run --user --scope`, caps each step's memory/CPU in its own child
  cgroup, and tears the whole subtree down atomically with the step's `cgroup.kill`. Real two-level
  boxing needs a Linux host with cgroup-v2 and a working systemd `--user` scope; where that is
  unavailable (laptops, most CI runners, macOS) a bare `run` **errors with exit 3** rather than
  silently running unprotected, and `--allow-cgroup-failure` downgrades to the safe `NoopCgroups`
  stand-in (no boxing; teardown falls back to a process-group kill) with a visible warning. The old
  opt-in `--cgroups` flag has been removed — boxing is ON by default, so passing it now errors (exit
  2). (The Python *library* entry point
  `run_dag(cgroups=None)` still defaults to `NoopCgroups`; it is the *CLI* that boxes by default —
  see the USER_GUIDE for enabling boxing from the API via `reexec_in_scope` + `Cgroups`.)
- **Memory-aware `-j` is wired into the CLI.** Pass `run --max-mem 8G` to have the runner pick the
  largest `-j` whose modeled worst-case footprint fits the budget, using the same memory model
  (`jobs_for_budget`, `schedulable_peak_mem_bytes`) available in the Python API. An explicit `--jobs`
  always overrides `--max-mem` (a note is printed when both are given). Per-step inner memory caps
  *are* applied automatically when cgroup boxing is active.
- **Per-step CSV metrics work.** The bundled file-backed `CsvMetricsSink` records the scheduler's
  per-step rows (one row per step) plus a whole-run summary row; use it from the CLI with
  `run --perf-dir DIR`, or pass `metrics=CsvMetricsSink(dir, git_sha=...)` to `run_dag` in the Python
  API. The header is derived from the actual row keys, so dynamic per-step columns (e.g. `cgroup
  cpu.*` counters, present only when cgroup boxing is active) are captured without configuration. The
  default remains a no-op sink that records nothing.
- **The Rust crate is at full parity.** `run`, `list`, `ascii`, `dot`, `json`, and `quickstart` all
  work in the Rust binary, and its `list`/`ascii`/`dot` output is byte-identical to the Python build,
  its `json` parsed-identical, and its `run` exit code + step counts identical — proven by the
  randomized `cross/differential.py` harness in CI. The Rust `run` **also boxes each step in a
  cgroup-v2 sandbox by default** (identical `--allow-cgroup-failure` opt-out and exit-3 behavior) and
  **also writes per-step + whole-run perf CSVs via `--perf-dir`**; the earlier Python-only gap for
  Linux cgroup boxing and perf logging has been closed.

## See also

- [`USER_GUIDE.md`](https://github.com/rrnewton/agent-utils/blob/main/common/docs/safe-ci-dag-runner/USER_GUIDE.md) — concepts, the complete DAG JSON schema, worked examples, the
  in-depth Python API, and troubleshooting.
- [`examples/`](https://github.com/rrnewton/agent-utils/tree/main/examples) — five small, runnable DAGs, one per core idea, each ready for `run --dag`.
- `safe-ci-dag-runner quickstart` — the same tour from the command line.

## License

MIT.
