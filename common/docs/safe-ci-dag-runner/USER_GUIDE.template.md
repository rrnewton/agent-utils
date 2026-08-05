# safe-ci-dag-runner user guide

`safe-ci-dag-runner` models build and test work as a DAG, plans it
deterministically, and executes ready steps concurrently. Dependencies, memory
budgets, named resource caps, timeouts, CPU reservations, and Linux containment
all constrain execution without changing the graph's meaning.

{{DISTRIBUTION}}

## A first DAG

Each step is identified by a `group.job` tag. `deps` names predecessor tags.
The optional `hint` object supplies estimates and limits; top-level
`resource_caps` limits caller-defined scarce resources.

```yaml
resource_caps:
  browser: 1
steps:
  - group: build
    job: app
    desc: compile the application
    cmd: make build
    hint:
      est_duration_s: 30
      classification: cpu-bound
      rss_baseline_bytes: 536870912
  - group: test
    job: unit
    cmd: make test
    deps: [build.app]
  - group: test
    job: browser
    cmd: ./run-browser-tests
    deps: [build.app]
    hint:
      resources: {browser: 1}
```

JSON and YAML express the same strict schema. File names ending in `.yaml` or
`.yml` select YAML; other paths select JSON. Use `--dag -` for JSON on standard
input. Unknown fields, invalid types, duplicate step tags, missing dependencies,
cycles, and impossible resource demands are rejected before execution.

## Inspect, convert, and visualize

```sh
safe-ci-dag-runner list --dag pipeline.yaml
safe-ci-dag-runner ascii --dag pipeline.yaml
safe-ci-dag-runner dot --dag pipeline.yaml > pipeline.dot
safe-ci-dag-runner json --dag pipeline.yaml > pipeline.json
safe-ci-dag-runner yaml --dag pipeline.json > pipeline.yaml
```

`list` is a compact inventory. `ascii` shows dependency layers. `dot` emits
Graphviz input. `json` emits canonical JSON, while `yaml` emits stable YAML.

## Plan without running

```sh
safe-ci-dag-runner plan --dag pipeline.yaml
safe-ci-dag-runner plan --dag pipeline.yaml --planner critical-path
safe-ci-dag-runner plan --dag pipeline.yaml --format json
```

The default `greedy-lpt` planner favors the longest ready step. The
`critical-path` planner favors the largest remaining weighted path. The `cpa`
planner additionally chooses inner widths from measured speedup curves. Stable
tag ordering breaks ties, so repeated plans over the same inputs are identical.

By default, past profile samples refine duration and resident-memory estimates.
Use `--no-profile-feedback` when only authored hints may influence a plan.

## Run safely

```sh
safe-ci-dag-runner run --dag pipeline.yaml -j 4
safe-ci-dag-runner run --dag pipeline.yaml --max-mem 8G
safe-ci-dag-runner run --dag pipeline.yaml --show-plan --profile
```

`-j` sets maximum outer concurrency. `--max-mem` instead chooses the largest
safe width from the modeled worst-case footprint. Named resources act as
semaphores in addition to the worker and memory limits. A failed step prevents
new dependent work; `--keep-going` lets already-running work reach a verdict.

Per-step wall and CPU timeouts, memory limits, process-tree teardown, and OOM
attribution are enforced inside nested cgroups when the host supplies cgroup v2
and a delegated systemd user scope. If that capability is missing, the default
is to stop with a capability error. `--allow-cgroup-failure` accepts a
best-effort unboxed fallback with a warning. `--unsafe-no-cgroups` deliberately
skips containment even when available and should be reserved for reviewed use.

Use `capabilities` for the machine-readable enforcement manifest.

## Select, parameterize, and stress a step

`--only` runs exactly the named tags, not their dependencies. It is useful for
profiling an artifact whose prerequisites already exist:

```sh
safe-ci-dag-runner run --dag pipeline.yaml --only test.unit
```

A command opts into passthrough arguments by including the reserved `{args}`
token:

```yaml
- group: test
  job: unit
  cmd: pytest {args}
```

```sh
safe-ci-dag-runner run --dag pipeline.yaml --only test.unit --args='-k retry'
```

Passing `--args` is rejected unless a selected command declares the token.
Without `--args`, the token is removed. `--stress N` creates `N` parallel
copies, implies keep-going, and reports the exact pass ratio. A stress request
whose modeled memory footprint does not fit the box is rejected before launch.

## Profiles, sweeps, and portable summaries

Runs append resource samples to `./.safe-ci-dag-runner/profiles/` by default.
Override the directory with `--perf-dir` or
`SAFE_CI_DAG_RUNNER_PROFILE_DIR`; disable writes with `--no-profile`. `--profile`
prints the current run's per-step table.

Measure one step across inner widths with:

```sh
safe-ci-dag-runner sweep --dag pipeline.yaml --step build.app --jobs 1..8
```

The `summary` command builds, merges, inspects, and plans from bounded portable
profile summaries. `run --profile-sync BACKEND` can download a shared summary
before planning and upload merged samples afterward. Run the relevant command
with `--help` for backend and direction syntax.

## Collision-free CPU reservations

For benchmark isolation, reserve a chosen number of least-busy allowed cores
and run a complete process tree on them:

```sh
safe-ci-dag-runner pin-run --cores 2 --tag parser-bench -- ./bench-parser
safe-ci-dag-runner run --dag benchmarks.yaml --cores 4
```

Reservations use a durable cross-process ledger, never choose cores held by a
concurrent reservation, release on normal or failing exit, and reclaim records
whose owning process is dead. Enforcement is fail-closed: the exact reserved set
must become the effective cgroup cpuset. A process-affinity mask is not accepted
because a descendant can replace it.

`pin-run` creates a transient `AllowedCPUs` scope and mutation-checks it before
launch. `run --cores K` changes only the runner's own verified scope, so it fails
with a capability result when combined with `--allow-cgroup-failure` or
`--unsafe-no-cgroups`. Reservation exhaustion also fails instead of choosing a
core held by another process.

The companion command exposes the allocator directly:

```sh
cpuset-alloc run --cores 2 --tag parser-bench -- ./bench-parser
cpuset-alloc status
cpuset-alloc reclaim
cpuset-alloc selftest
```

The `--` separator before the wrapped command is required. `selftest` directly
attempts to move a child onto an excluded CPU and verifies every assigned CPU is
usable; a missing or inconclusive mutation is a failure, not evidence of a hard
bound. CPU pinning is intended for controlled measurements, not ordinary CI.

## Command summary

| Command | Purpose |
|---|---|
| `run` | Execute a DAG under scheduling and containment constraints. |
| `plan` | Show estimates, critical path, widths, and order without running. |
| `list` / `ascii` / `dot` | Inspect or visualize the graph. |
| `json` / `yaml` | Validate and convert the DAG. |
| `sweep` | Measure one step across inner job widths. |
| `summary` | Build, merge, inspect, or consume portable profile summaries. |
| `pin-run` | Reserve disjoint cores and run one command tree. |
| `capabilities` | Print the enforcement-capability manifest. |
| `quickstart` | Print a runnable introduction. |

All commands support `--help`; the top level supports `--version` and
`--userguide`.

## Exit behavior

Successful inspection and all-passing runs exit zero. Invalid input or command
usage is nonzero. A run with a failed step is nonzero. Failure to establish
required containment has a distinct nonzero capability result. `pin-run` and
`cpuset-alloc run` return the wrapped command's exit status after releasing the
reservation; signal termination uses the conventional `128 + signal` status.

## License

MIT
