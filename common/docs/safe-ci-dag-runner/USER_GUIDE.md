# safe-ci-dag-runner — User Guide

This guide goes deeper than the [README](README.md): the core concepts, the complete DAG JSON
schema, worked examples, the full Python API, and troubleshooting. If you just want to get running,
start with the README's 60-second quickstart or run `safe-ci-dag-runner quickstart`.

## Contents

- [Concepts](#concepts)
- [Planning algorithm](#planning-algorithm)
- [The DAG JSON schema](#the-dag-json-schema)
- [YAML: an isomorphic, literate alternative to JSON](#yaml-an-isomorphic-literate-alternative-to-json)
- [Worked examples](#worked-examples)
- [Profiling and experimenting with individual steps](#profiling-and-experimenting-with-individual-steps)
- [The Python API in depth](#the-python-api-in-depth)
- [Enabling real cgroup boxing](#enabling-real-cgroup-boxing)
- [Troubleshooting](#troubleshooting)

## Concepts

**The resource model in one glance — three independent parts.** Before the individual concepts, the
big picture: the runner reasons about three *separate* kinds of resource, and it helps to keep them
distinct.

1. **Memory (bytes / GB)** — per-step `rss_baseline_bytes` / `hard_mem_max_bytes` hints drive
   memory-aware `-j` (the largest concurrency whose modeled worst-case RAM fits `--max-mem`) and the
   per-step inner memory cap under cgroup boxing. Usually the first knob you reach for.
2. **CPU** — the outer `-j` (how many steps run at once), a step's own `preferred_inner_jobs` width,
   and (under boxing) CPU caps carved from the delegated cgroup scope.
3. **Named scarce resources** — arbitrary caller-named semaphores (`hint.resources` bounded by
   `resource_caps`) for anything memory and CPU do not capture, such as "only one browser at a time".

The concepts below cover each in turn.

**DAG.** Your build/test pipeline is a directed acyclic graph: a set of steps, where an edge means
"this step depends on that one." The runner executes steps concurrently, only starting a step once
all of its dependencies have finished successfully.

**Step.** One node in the graph: a shell command (`cmd`, run via `bash -c`) plus its dependencies and
an optional resource hint. Every step is identified by its fully-qualified **tag**, `group.job` (for
example `build.app`). Tags must be unique — they are how dependencies, resource accounting, and
per-step boxing all refer to a step. The `group` is simply a **namespace** for organizing and
referring to related steps (for example every `e2e.*` step); it carries no scheduling meaning of its
own.

**Dependencies.** A step lists the tags it depends on in `deps`. A step launches only once **every**
dependency has a *done and successful* outcome. If any dependency **fails**, the step is never run —
and the failure closes transitively, so everything downstream of a failure is marked `skipped`.

**resource_caps.** A top-level map bounding the total concurrent demand for each named scarce
resource. The runner never lets the summed `resources` demand of the steps running at one instant
exceed the cap. The classic use is `{"browser": 1}` to **serialize** browser/e2e steps (only one at a
time) even while other steps run in parallel. (It is introduced here *before* the per-step
`ResourceHint` because the caps are what a step's `hint.resources` demand is measured against.)

Resource names are **arbitrary strings you choose — not a built-in list**: `"browser"`, `"db"`,
`"gpu"`, `"api-tokens"`, anything. The only rule is that every resource a step demands in its hint
must have a matching key in `resource_caps` with enough capacity; demand an uncapped (or
insufficiently-capped) resource and that step can never be scheduled, stalling the run. `"browser"`
is *only a name*: DeepScry uses it to serialize its Playwright/browser end-to-end tests, which each
grab fixed ports and a display and so cannot run concurrently — but the runner has no built-in notion
of a browser. (The single place the literal name `browser` is recognized is a *cosmetic display*
default — a browser-resource step is shown as `latency-bound` unless you set a class; see the
classification auto-promotion note under the schema below. It changes no scheduling.)

**ResourceHint.** Optional per-step scheduling knowledge attached to a step. Every field is optional;
supplying them unlocks smarter behavior:

- `resources` — how much of each *scarce named resource* the step needs (e.g. `{"browser": 1}`),
  matched against the `resource_caps` above.
- `est_duration_s` — an estimated runtime, used only to *order* ready steps.
- `rss_baseline_bytes` / `hard_mem_max_bytes` — memory estimates that feed the memory model and the
  per-step inner memory cap.
- `classification` — `cpu-bound`, `latency-bound`, or `light`.
- `preferred_inner_jobs` — the step's own internal parallelism width.

**Longest-first (LPT) scheduling.** When several steps are ready and a slot frees up, the runner
picks the one with the largest `est_duration_s` first (a stable sort, so steps with equal or no
estimate keep their registration order). This "longest processing time" heuristic keeps big steps off
the tail of the run, shortening overall wall-clock time. It only decides *ordering*; dependency and
resource gating are enforced independently.

**Memory-aware `-j`.** Rather than a flat "N GB per job" guess, the memory model enumerates which
steps could *actually* co-run — no transitive dependency between them, and their summed resource
demand fits the caps — and takes the worst-case sum of their per-step memory caps. That yields an
exact "largest `-jN` that fits budget M." Use it from the CLI with `run --max-mem 8G`, or from the
Python API with the `jobs_for_budget` helper. The CLI's default `-j` (when neither `--jobs` nor
`--max-mem` is given) is the CPU count; an explicit `--jobs` always overrides `--max-mem`.

Two facts keep `--max-mem` from being surprising. First, "CPU count" everywhere here means
**logical** CPUs (`os.cpu_count()` — SMT/hyperthreads included), which is both the plain default `-j`
and the ceiling `--max-mem` sizing is capped at. Second, memory-aware sizing only bites when steps
actually carry `rss_baseline_bytes`: a DAG whose steps have **no** memory baselines has a modeled
footprint of `0`, which is then clamped up to `mem_cap_floor_bytes` (default 8 GiB). So for any budget
at or above that floor the model concludes every `-j` up to the CPU count fits, `--max-mem` picks the
full CPU count, and it does **not** throttle. The CLI prints a one-line note whenever this happens
(chosen `-j` equals the CPU count AND no step has a baseline), so an un-throttled run is never a
silent surprise; add per-step `rss_baseline_bytes` hints to make the budget actually constrain `-j`.

**cgroup boxing.** On a Linux cgroup-v2 host with a *delegated* hierarchy, the runner puts the whole
run in an outer CPU/memory box and each step in its own nested box — and the CLI `run` does this **by
default** (it re-execs into a transient `systemd-run --user --scope`; see [Enabling real cgroup
boxing](#enabling-real-cgroup-boxing)). The payoff is twofold: (1) a step that exceeds its memory cap
is OOM-killed in isolation instead of taking down the host; (2) teardown writes the step's
`cgroup.kill`, an **atomic SIGKILL of the entire subtree** that catches `setsid`/double-fork escapees
(orphan servers, browsers) a process-group kill misses. Where a delegated cgroup is unavailable a
bare `run` **errors with exit 3**; `--allow-cgroup-failure` downgrades to a process-group kill and
says so out loud.

**Metrics sink.** An optional pluggable destination for durable measurements: one whole-run summary
row (wall time, CPU contention split, cores) and per-step rows (CPU, memory peak, threads, ambient
load bucket). The default records nothing. Metrics never fail a run.

**Visualization.** `to_dot` renders Graphviz DOT (one cluster per group, solid dependency edges, and
a dashed chain across the users of each cap-1 resource to hint that they serialize). `to_ascii`
renders a compact topological-layer view for the terminal.

## Planning algorithm

The scheduler today is a single **greedy, longest-processing-time-first (LPT)** pass over the ready
set. On every sweep it launches each step that is *ready* — all dependencies finished successfully,
its scarce-resource demand fits the remaining `resource_caps`, and the run is under its `-j` fan-out —
considering ready steps in descending `est_duration_s`, so that when a slot or a scarce resource
frees, the heaviest ready step claims it. That keeps long steps off the tail of the run and shortens
overall wall-clock time. Ordering is the *only* thing the estimate affects; dependency and resource
gating are enforced independently, so a wrong estimate can mildly degrade packing but never breaks
correctness.

Two planned improvements are worth knowing about:

- **A pluggable planner (`--planner`).** The greedy LPT pass is the only strategy for now; a
  `--planner` flag that selects a smarter **critical-path / lookahead** planner — one that reasons
  about the whole remaining graph, not just the current ready set — is planned (ds-afzsqf).
- **A learned duration profile.** `est_duration_s` is, for now, a static hint you write into the DAG.
  A separate, auto-updated **profile store** that records real per-step durations from past runs and
  feeds them back as the estimate is planned (ds-7pzdgm). Its **storage** half now ships: every `run`
  and `sweep` auto-logs per-step timings and memory to a default profile store (see
  [Profiling and experimenting with individual steps](#profiling-and-experimenting-with-individual-steps)
  below), so the data a future planner would learn from is already being collected. Feeding those
  recorded durations back in as the estimate automatically is the remaining step.

## The DAG JSON schema

A DAG file is a single JSON object. Parsing is **strict**: a wrong-typed or malformed field is a hard
error (exit `2`), never a silent default. Only `steps`, and within each step `group`/`job`/`cmd`, are
required; everything else defaults. `safe-ci-dag-runner json --dag FILE` re-emits any DAG with every
field filled in, which is the easiest way to see the defaults applied to your file.

### Top-level fields

| Field                    | Type                | Default            | Meaning                                                                                                   |
| ------------------------ | ------------------- | ------------------ | --------------------------------------------------------------------------------------------------------- |
| `steps`                  | array of step       | — (required)       | The graph's nodes. May be empty (`[]`), which is a valid, no-op DAG.                                       |
| `description`            | string              | `""`               | Optional long-form documentation for the **whole DAG** (free-form prose, often multi-line). Purely documentary — never affects scheduling. In YAML it is naturally a block scalar (see [YAML](#yaml-an-isomorphic-literate-alternative-to-json)). |
| `resource_caps`          | object `{str: int}` | `{}`               | Capacity per named scarce resource. Concurrent summed `hint.resources` demand never exceeds this.         |
| `mem_cap_factor`         | number              | `1.25`             | Multiplier from a step's `rss_baseline_bytes` to its derived inner memory cap (headroom).                  |
| `mem_cap_floor_bytes`    | integer             | `8589934592` (8 GiB) | Lower bound on the modeled worst-case footprint, so `-j` selection never concludes "0 fits."             |
| `outer_mem_safety_factor`| number              | `1.0`              | Multiplier applied to the modeled peak footprint (`1.0` = no inflation).                                   |
| `default_step_timeout`   | integer             | `1800`             | Default per-step timeout (seconds) carried as caller policy. Note: the runner enforces each step's own `timeout`. |
| `default_jobs_flag`      | string              | `"-j"`             | Default inner-jobs flag template for steps that declare `preferred_inner_jobs` but do not set their own `jobs_flag` (see the step `jobs_flag` field). |

### Step fields

| Field         | Type                | Default        | Meaning                                                                                                      |
| ------------- | ------------------- | -------------- | ----------------------------------------------------------------------------------------------------------- |
| `group`       | string              | — (required)   | First half of the tag.                                                                                       |
| `job`         | string              | — (required)   | Second half of the tag. The full tag `group.job` must be unique across the DAG.                             |
| `cmd`         | string              | — (required)   | Shell command, run via `bash -c` from the current working directory, in its own session/process group.      |
| `desc`        | string              | `""`           | Short human-readable label shown by `list` and `run`.                                                        |
| `description` | string              | `""`           | Optional long-form documentation for this node (free-form prose, often multi-line). Distinct from the short `desc` label; purely documentary and never affects scheduling. Renders naturally as a YAML block scalar. |
| `deps`        | array of string     | `[]`           | Tags this step depends on. All must finish successfully before it launches.                                  |
| `env`         | object `{str: str}` | `{}`           | Extra environment variables, merged over the runner's environment for this step's command.                  |
| `hint`        | object              | `{}`           | The resource hint (see below).                                                                               |
| `jobs_flag`   | string or `null`    | `null` (inherit) | Template for the inner-jobs flag appended to `cmd` when the step declares `preferred_inner_jobs`. `null` inherits the DAG's `default_jobs_flag`; `""` **disables** the append (the command manages its own concurrency). Spellings: `"-j"` → `-j 8`, `"-j%d"` → `-j8`, `"--jobs="` → `--jobs=8`, `"--num-threads"` → `--num-threads 8`. |
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
| `preferred_inner_jobs`     | integer or `null`   | `null`     | The step's own internal parallelism width. Sets the inner CPU cap when boxed, scales the memory-budget model, and — unless suppressed by `jobs_flag` — is rendered into an inner-jobs flag **appended to the step's `cmd`** (see the `jobs_flag` step field). |
| `measured_effective_cores` | number or `null`    | `null`     | Measurement passthrough (recorded in metrics; not used for scheduling).                                       |
| `measured_cpu_utilization` | number or `null`    | `null`     | Measurement passthrough (recorded in metrics; not used for scheduling).                                       |

> **Classification auto-promotion is a runtime/display derivation, not stored state.** A step that
> demands the `browser` resource but leaves `classification` unset is *scheduled and shown* as
> `latency-bound`: `list`, `ascii`, and `dot` render `[latency-bound]` for it. That promotion is
> computed on the fly (`step_classification`) from the step's resource demand — it is **not** written
> back onto the step. So the canonical JSON (`safe-ci-dag-runner json`, and any round-trip through
> `dag_to_json`) re-emits the *stored* value — `"light"` for such a step — not the promoted
> `"latency-bound"`. This is intentional: the JSON is the exact input you supplied, while the
> promotion is a scheduling view derived from it. To pin `latency-bound` into the canonical JSON, set
> `classification` explicitly on the hint.

## YAML: an isomorphic, literate alternative to JSON

A DAG can be written in **YAML** instead of JSON. YAML is **isomorphic** to the JSON schema: every
field, default, and validation rule is identical — the two formats are parsed into the *same* model
through the *same* strict narrowing, so anything you can express in a JSON DAG you can express in a
YAML DAG and vice versa. The only differences are surface syntax and two conveniences YAML adds.

**Auto-detection by extension.** `--dag FILE` chooses the format from the file extension:
`.yaml`/`.yml` load as YAML, everything else as JSON. Reading from stdin (`--dag -`) always assumes
JSON. So no flag is needed — `run`, `list`, `ascii`, `dot`, `json`, and `yaml` all accept either
format transparently:

```sh
safe-ci-dag-runner run  --dag pipeline.yaml --allow-cgroup-failure
safe-ci-dag-runner list --dag pipeline.yml
```

**Two conveniences that make DAGs "literate".** YAML allows:

1. **Comments** (`# ...`) — annotate the pipeline inline, right next to the steps.
2. **Multi-line block scalars** — write a long `description` across several lines with a literal
   (`|`) or folded (`>`) block scalar, instead of cramming `\n` escapes into a JSON string.

Together these let a DAG document *why* each step exists, in prose, in the file itself. The
`description` field (top-level and per-node — see the schema tables above) is the natural home for
this. For example:

```yaml
# The whole pipeline, documented inline.
description: |-
  Build the app, then run the checks in parallel, then deploy.
  Deploy waits for every check to pass.
steps:
  - group: build
    job: app
    desc: compile            # short label shown by `list`/`run`
    description: |-           # long-form docs: a literal block scalar keeps the line breaks
      Compile the application once.
      Both downstream checks depend on this single artifact.
    cmd: make build
```

**Emitting YAML.** The `yaml` subcommand re-emits any loaded DAG as YAML (mirroring how `json`
re-emits canonical JSON):

```sh
safe-ci-dag-runner yaml --dag pipeline.json    # JSON -> YAML
safe-ci-dag-runner json --dag pipeline.yaml    # YAML -> canonical JSON
```

Loading is guaranteed isomorphic across the Python and Rust builds (a YAML file loads to a
byte-identical canonical JSON under both — enforced by `cross/differential.py`). The *emitted* YAML
bytes are not guaranteed identical across the two builds; only loading is. A JSON DAG and its YAML
translation always load to the same DAG.

**Watch the "Norway problem" (quote ambiguous scalars).** In YAML, an *unquoted* `no`, `yes`, `on`,
or `off` parses as a **boolean**, and an unquoted `true`/`false` or a bare number parses as a
bool/number — not a string. If you mean the literal *string* `"no"` (or a command like `true`, or a
numeric-looking id), **quote it**: `desc: "no"`, `cmd: "true"`, `job: "123"`. The strict parser will
otherwise reject a boolean where it wants a string (e.g. `cmd` must be a string) — loudly, never
silently. The Python (PyYAML) and Rust (serde_norway) parsers agree on all quoted forms; the
adversarial fixture in `cross/yaml_fixtures/` pins this.

**The `serde_norway` crate (Rust side).** The Rust build parses YAML with
[`serde_norway`](https://crates.io/crates/serde_norway) — the maintained fork of the archived
`serde_yaml`, keeping the same serde API and YAML 1.2 core-schema semantics. It deserializes YAML
into the same `serde_json::Value` intermediate the JSON path uses, so both syntaxes build the model
through one code path.

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

A build that runs its own `-j` internally. Declare the width so the runner can append the inner-jobs
flag, set an inner CPU cap (when boxed), and account for the extra memory in the budget model:

```json
{
  "steps": [
    {"group": "build", "job": "app", "desc": "parallel compile",
     "cmd": "make build",
     "hint": {"classification": "cpu-bound", "preferred_inner_jobs": 8,
              "rss_baseline_bytes": 6442450944}}
  ]
}
```

`preferred_inner_jobs: 8` does two things. First, the runner **appends** the inner-jobs flag to your
command: with the default `-j` template the step actually runs as `make build -j 8`, so you declare
the width once instead of hardcoding it in `cmd`. (Set `jobs_flag` to change the spelling — e.g.
`"-j%d"` → `make build -j8` — or `""` to opt out when your command already sets its own `-j`.) Second,
it feeds scheduling: when cgroup boxing is active the step is capped to 8 CPUs, and in the memory
model a `cpu-bound` step's cap scales with its inner width above a width of 4.

## Profiling and experimenting with individual steps

The whole-DAG `run` is the common path, but three tools let you profile and experiment with a
**single** step, and make the resulting profiling data land somewhere obvious so it can be browsed
and (later) feed planning.

### `run --only TAG[,TAG...]` — run exactly the named step(s)

`--only` runs *exactly* the named steps and nothing else. Crucially, it does **not** run their
dependencies: dependency edges pointing to steps *outside* the selection are dropped, and those
inputs are assumed to already be present. This is for iterating on or profiling one step without
paying to rebuild the whole graph.

```sh
safe-ci-dag-runner run --dag dag.json --only build.app                 # one step
safe-ci-dag-runner run --dag dag.json --only build.app,test.unit       # several, comma-separated
```

Edges *among* the selected steps are preserved, so a selected sub-graph still runs in the right
order. An unknown tag is a hard error (exit `2`) that lists the known tags. Both the Python and Rust
builds filter identically (enforced by `cross/differential.py`).

### `run --profile` — a per-step profile table after the run

Add `--profile` to print a per-step table once the run finishes, so a CI-optimizing agent sees what
happened without opening a CSV:

```
per-step profile:
step       wall_s  user_s  sys_s  rss_hwm  oom  inner_jobs
---------  ------  ------  -----  -------  ---  ----------
build.app   0.920   3.438  0.004  2.6 MiB    0           4
test.unit   0.269   0.002  0.016  512.0 KiB  0     ambient
```

`user_s`/`sys_s` come from each step's cgroup `cpu.stat` and `rss_hwm` from its `memory.peak`, so
they are populated under real boxing and shown as `-` in an un-boxed (`--allow-cgroup-failure`) run.

### `sweep` — a per-step parallel-speedup study

`sweep` runs ONE step at inner parallelism `-j1, -j2, … -jN`, passing each width into the step's
command through its `jobs_flag` (so, e.g., `jobs_flag: "-j%d"` makes width 4 arrive as `-j4`), and
prints the classic parallel-speedup table:

```
$ safe-ci-dag-runner sweep --dag dag.json --step build.app --jobs 1..8
parallel-speedup sweep: build.app
jobs  wall_s  user_s  sys_s  rss_hwm  speedup(vs j1)
----  ------  ------  -----  -------  --------------
1      3.574   3.514  0.007  1.2 MiB           1.00x
2      1.770   3.432  0.003  1.6 MiB           2.02x
8      0.519   3.553  0.020  3.7 MiB           6.89x
```

Flags: `--step TAG` (required), `--jobs RANGE` (`LO..HI` like `1..8`, or a bare `N` meaning `1..N`),
`--repeat K` (run each width K times and keep the fastest wall time; default `1`), and the same
`--perf-dir` / `--no-profile` / `--allow-cgroup-failure` / `-v` as `run`. Like `run`, `sweep` boxes
each measurement by default, so it measures under real cgroup limits. (Runtimes legitimately differ
between the Python and Rust builds, so the sweep/`--profile` *table contents* are not byte-compared
across languages — but the `--only` selection semantics, the profile-store schema, and the
`json`/`list`/`ascii`/`dot` output all are.)

### The default profile store (auto-logging)

Every `run` and `sweep` **auto-logs** resource-usage CSVs; you do not need `--perf-dir`. The
destination is resolved in order:

1. `--no-profile` — disable logging.
2. `--perf-dir DIR` — explicit directory (wins over the rest).
3. `$SAFE_CI_DAG_RUNNER_PROFILE_DIR` — an environment override.
4. **Default:** `./.safe-ci-dag-runner/profiles/`, repo-local (relative to the CWD), created on
   demand.

The tool always prints where it appended (No Silent Failure). The files and schema are exactly what
`--perf-dir` has always written — `<machine_id>.csv` (one whole-run summary row) and
`step_profiles_<machine>_<container>.csv` (one row per step, including the `inner_jobs` width and the
dynamic cgroup `cpu.*` columns under boxing) — so a sweep's rows are browsable per width. Consider
**gitignoring** `./.safe-ci-dag-runner/` (machine-local perf data, not source), or check it in to
keep a history — your choice.

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

`StepClass` is the classification enum; set it on a hint to pin how a step is scheduled instead of
relying on the `light`-or-auto-promoted default:

```python
from safe_ci_dag_runner import ResourceHint, StepClass

hint = ResourceHint(classification=StepClass.CPU_BOUND)   # or StepClass.LATENCY_BOUND / StepClass.LIGHT
```

Its three members map one-to-one onto the JSON `classification` strings — `StepClass.CPU_BOUND` ↔
`"cpu-bound"`, `StepClass.LATENCY_BOUND` ↔ `"latency-bound"`, `StepClass.LIGHT` ↔ `"light"` (each
`member.value` is exactly that string) — so a Python `DagConfig` and its serialized JSON agree.

### `run_dag`

```python
run_dag(
    cfg: DagConfig,
    *,
    jobs: int,                       # outer fan-out (-j); clamped to >= 1
    cgroups: CgroupManager | None = None,   # None -> NoopCgroups (no boxing)
    metrics: MetricsSink | None = None,      # None -> record nothing
    keep_going: bool = False,        # on failure, let running steps finish (still stops launching new)
    verbosity: int = 1,              # 0 quiet(+failures), 1 default(+summaries), >=2 stream child stdout
) -> RunResult
```

`jobs` is required and keyword-only. Note the split between the two entry points: the **library**
`run_dag(cgroups=None)` defaults to `NoopCgroups` (no boxing), whereas the **CLI** `run` boxes by
default and only runs un-boxed when you pass `--allow-cgroup-failure` (see [Enabling real cgroup
boxing](#enabling-real-cgroup-boxing)). Passing a `cgroups` manager whose `.enabled` is `False`
triggers a visible "containment is DEGRADED" warning (No Silent Failure) and runs un-boxed. Passing a
real `metrics` sink but getting nothing recorded also warns.

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

From the CLI, pass a budget and let the runner size `-j` (add `--allow-cgroup-failure` on a host
without cgroups, or drop it on a Linux systemd host for real boxing):

```sh
safe-ci-dag-runner run --dag dag.json --max-mem 8G --allow-cgroup-failure
```

It picks the largest `-j` whose modeled worst-case footprint fits `8G` and prints the decision. An
explicit `--jobs` overrides `--max-mem` (with a note). The same is available in the API:

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

A file-backed `CsvMetricsSink` also ships and works out of the box. It writes one CSV row per step
plus a whole-run summary row into a directory you choose:

```python
from safe_ci_dag_runner import run_dag, CsvMetricsSink

sink = CsvMetricsSink("./perf", git_sha="deadbeef")   # git_sha stamps every row
run_dag(cfg, jobs=4, metrics=sink)
# ./perf/step_profiles_<machine>_<container>.csv  (one row per step; columns derived from the
#                                                  actual row keys, incl. dynamic cgroup cpu.* )
# ./perf/<machine>.csv                            (one whole-run summary row: wall time, CPU
#                                                  contention split, cores, result)
```

The CLI wraps this as `run --perf-dir ./perf` (available in both the Python and Rust builds). The
`run_dag` library default remains `metrics=None` (record nothing). Note the dynamic `cpu.*` per-step
columns only appear when cgroup boxing is active — i.e. an actual boxed `run` (the default on a Linux
systemd host), not an `--allow-cgroup-failure` un-boxed run; without boxing those cgroup-derived
measurements are unavailable and the CSV simply omits them.

### Plugging in cgroup containment

`run_dag(cgroups=...)` accepts anything satisfying the `CgroupManager` protocol. Two are bundled:

- `NoopCgroups` — the safe default (`enabled == False`): no boxing, teardown falls back to a
  process-group kill. Works on any host.
- `Cgroups` — the real Linux cgroup-v2 manager. It is `enabled` **only** when the process is already
  inside a delegated cgroup-v2 scope. To set that scope up (and turn boxing on), follow the runnable
  recipe in [Enabling real cgroup boxing](#enabling-real-cgroup-boxing) below.

When boxing is active, each step's inner memory cap comes from its hint (`hard_mem_max_bytes`, else
`rss_baseline_bytes` × `mem_cap_factor`) and its inner CPU cap from `preferred_inner_jobs`.

## Enabling real cgroup boxing

The CLI `run` boxes each step **by default**: it performs the `reexec_in_scope` step described below
for you, and errors with exit 3 where cgroup-v2 + a systemd `--user` scope are unavailable (unless
you pass `--allow-cgroup-failure` to run un-boxed). The **library** entry point `run_dag(cgroups=None)`
instead defaults to `NoopCgroups` (no boxing). This section shows how to turn real boxing on from the
Python API — the same mechanism the CLI uses under the hood.

### Prerequisites

Real two-level boxing needs all of:

- **Linux with the cgroup-v2 unified hierarchy** mounted at `/sys/fs/cgroup` (not the legacy
  cgroup-v1 or hybrid layout).
- **A delegated scope carrying the `cpu` and `memory` controllers.** The normal way to get one is a
  systemd **user** session where `systemd-run --user --scope` works: the runner launches the outer
  scope with `-p Delegate=yes` so it can carve per-step child cgroups underneath. (In a systemd-less
  container whose namespace cgroup root already delegates `cpu`+`memory`, the systemd-free
  `enter_delegated_scope` fallback reaches the same end state.)

You can check the two ingredients directly:

```sh
stat -fc %T /sys/fs/cgroup                 # -> cgroup2fs  (the unified v2 hierarchy)
systemd-run --user --scope --quiet true    # exits 0 iff a --user scope works here
```

If either is missing, `Cgroups().enabled` is `False` and the runner correctly falls back to a
process-group kill.

### Recipe: box a run via `reexec_in_scope`

The entry point is `safe_ci_dag_runner.cgroup.reexec_in_scope`. It re-execs the current process
inside a transient, delegated `systemd-run --user --scope`, so every descendant — including
`setsid`/double-forked escapees — is contained and reaped atomically. Save this as `boxed_run.py`:

```python
import sys
from safe_ci_dag_runner import Step, DagConfig, run_dag
from safe_ci_dag_runner.cgroup import reexec_in_scope, Cgroups

cfg = DagConfig(steps=(Step("build", "app", "compile", "echo build && sleep 0.1"),))

# Re-exec THIS process inside a transient, delegated systemd --user scope.
#   - On the FIRST call, on success, execvp REPLACES the process and does NOT return; the
#     program restarts from the top, now running INSIDE the scope.
#   - On that second pass the SAFE_CI_IN_SCOPE sentinel is set, so reexec_in_scope() returns
#     True (anti-recursion) and execution falls through to here.
#   - It returns False only when a --user scope is unavailable or the exec failed.
# Pass memory_max=<bytes> and/or cpu_count=<n> to cap the whole run's outer box.
if not reexec_in_scope(sys.argv, memory_max=None, skip_in_ci=False):
    sys.exit("refusing to run without cgroup enforcement (no delegated cgroup-v2 scope)")

cg = Cgroups()                          # the real manager; construct it INSIDE the scope
if not cg.enabled:
    sys.exit("cgroup boxing did not engage")
print("Cgroups().enabled ==", cg.enabled)   # -> True
run_dag(cfg, jobs=4, cgroups=cg)
```

```sh
python3 boxed_run.py
```

`reexec_in_scope` defaults to `skip_in_ci=True`, which returns `True` **without** re-execing when
`CI` or `GITHUB_ACTIONS` is set (so a plain CI run is not boxed); pass `skip_in_ci=False`, as above,
to force boxing even under CI. Its other keyword arguments: `memory_max` (outer `MemoryMax` in bytes;
swap is always disabled regardless), `cpu_count` (outer `CPUQuota`), `naming` (a `ScopeNaming` value
to brand the scope/slice/log-prefix), and `use_aggregate_slice` (share one CPU cap across concurrent
runs).

### Verifying it engaged

`Cgroups().enabled is True` is the one-line "boxing is live" signal, and the recipe above prints it.
For a fuller audit of the outer box after the re-exec, call
`verify_scope_limits(expected_memory_max, expected_cpu_count)` — it reads back the scope's
`memory.max`, `memory.swap.max`, and `cpu.max` and prints `bound`/`MISMATCH` evidence, returning
`True` only when the requested limits actually reached cgroup-v2.

When boxing is active, each step's inner memory cap comes from its hint (`hard_mem_max_bytes`, else
`rss_baseline_bytes` × `mem_cap_factor`) and its inner CPU cap from `preferred_inner_jobs`; a step
that exceeds its inner `memory.max` is OOM-killed in isolation, and teardown uses the step cgroup's
`cgroup.kill` (setsid-proof) instead of a process-group kill.

### The CLI boxes by default

The CLI `run` performs the outer `reexec_in_scope` step for you: on a Linux host with cgroup-v2 and a
working systemd `--user` scope, a bare `run` re-execs into a transient delegated scope and boxes every
step — no wrapper needed. Where that environment is unavailable it **errors with exit 3** rather than
silently running unprotected; pass `--allow-cgroup-failure` to run un-boxed with a visible warning.
The old opt-in `--cgroups` flag is now a deprecated no-op (accepted for backward compatibility), and
the Python and Rust builds behave identically here. Use the `reexec_in_scope` + `Cgroups` recipe above
when you want the same turn-key boxing from your own Python program rather than the CLI.

## Troubleshooting

**"containment is DEGRADED" / steps run un-boxed.** This is a **library**-path signal: you passed
`run_dag` a `CgroupManager` whose `.enabled` is `False` (e.g. a `Cgroups()` constructed outside a
delegated scope). The runner prints, then continues un-boxed:

```
[scheduler] ⚠ per-step cgroup manager is present but disabled; containment is DEGRADED (falling back to process-group kill for teardown, no inner memory/CPU caps).
```

It means the process is **not** inside a delegated cgroup-v2 scope (a non-Linux host, no systemd
`--user` delegation, or you didn't set up the outer scope). Steps still run correctly; teardown just
uses a process-group kill instead of `cgroup.kill`, and no inner memory/CPU caps are applied. To get
real boxing, enter a delegated scope via `reexec_in_scope` (see above). The warning is deliberate —
the tool never silently drops a cap you asked for. From the **CLI** you won't see this line: a bare
`run` either boxes successfully or errors with exit 3 where boxing is unavailable, and
`--allow-cgroup-failure` runs un-boxed with its own `cgroup boxing not established
(--allow-cgroup-failure); running UNBOXED` warning instead.

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

**`CsvMetricsSink` and the `--perf-dir` CSVs.** The bundled file-backed CSV sink works (this was
broken previously and is now fixed): `run_dag(cfg, metrics=CsvMetricsSink(dir, git_sha=...))`, or the
CLI `run --perf-dir DIR`, writes a per-step CSV and a whole-run CSV, creating the directory and files
as needed. The per-step CSV's columns are derived from the actual row keys, so it never drops data.
The dynamic `cpu.*` per-step columns only appear when cgroup boxing is active (see the "containment
is DEGRADED" note above): the CLI's `--cgroups` only boxes steps when the process is already inside a
delegated cgroup-v2 scope, so on a plain host those cgroup-derived measurements are simply absent from
the CSV rather than an error.

**Python vs Rust.** The Rust binary is at **full parity** with the Python build: `run`, `list`,
`ascii`, `dot`, `json`, and `quickstart` all work, with `list`/`ascii`/`dot` output byte-identical to
the Python build, `json` parsed-identical, and `run` exit code + step counts identical (enforced by the
`cross/differential.py` harness). The Rust `run` also **boxes each step in a cgroup-v2 sandbox by
default** (identical `--allow-cgroup-failure` opt-out and exit-3 behavior) and **writes per-step +
whole-run perf CSVs via `--perf-dir`**; the earlier Python-only gap for Linux cgroup boxing, perf
logging, and ambient-load bucketing has been closed. `--cgroups` is a deprecated no-op in both builds.
Either package works — install the Python one with
`pip install "git+https://github.com/rrnewton/agent-utils#subdirectory=py"`, or build the Rust crate
under `rs/`.
