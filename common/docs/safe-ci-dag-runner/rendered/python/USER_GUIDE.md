# safe-ci-dag-runner user guide

`safe-ci-dag-runner` models build and test work as a DAG, plans it
deterministically, and executes ready steps concurrently. Dependencies, memory
budgets, named resource caps, timeouts, CPU reservations, and Linux containment
all constrain execution without changing the graph's meaning.

## Installation and library use

```sh
python3 -m pip install safe-ci-dag-runner
```

Python 3.10 or newer is required. Installation provides both console commands,
the typed `safe_ci_dag_runner` package, and this guide as package data.

```python
from safe_ci_dag_runner import DagConfig, Step, to_ascii

dag = DagConfig(steps=(Step("build", "app", "compile", "make build"),))
print(to_ascii(dag))
```

The package also exports strict JSON/YAML conversion, resource sizing,
pluggable containment and metrics protocols, plan construction, and profile
analysis. The `run_dag` library function is an explicit process-group-only
scheduler unless the caller supplies an enabled `CgroupManager`. Use the
console command on Linux with cgroup v2 and a delegated systemd user scope when
the package should establish and verify containment for you.

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

### Fail closed on protected artifact writes

An opt-in `write_domain_policy` turns per-step write declarations into a
pre-execution requirement. `allowed_domains` is a closed vocabulary;
`require_explicit: true` requires every step to carry `write_domains` (use `[]`
when the step writes none of the protected domains). Unknown, duplicate, or
missing domains stop the whole DAG before the first command starts.

Every non-empty declaration also names its structural guarantee:

- `artifact-producer` creates mutable inputs to a later publisher.
- `immutable-artifact-barrier` atomically publishes the immutable artifact.
- `artifact-barrier-dependent` must transitively depend on such a barrier; the
  runner verifies that graph relationship before execution.
- `explicitly-isolated` writes a package/path-disjoint output.

```yaml
write_domain_policy:
  require_explicit: true
  allowed_domains: [shared-target, isolated-target]
steps:
  - group: build
    job: publish
    cmd: ./publish-immutable
    write_domains: [shared-target]
    write_domain_guarantee: immutable-artifact-barrier
  - group: test
    job: unit
    cmd: ./run-shared-target-test
    deps: [build.publish]
    write_domains: [shared-target]
    write_domain_guarantee: artifact-barrier-dependent
  - group: build
    job: fixture
    cmd: ./build-fixture-in-private-target
    write_domains: [isolated-target]
    write_domain_guarantee: explicitly-isolated
```

Write domains are not scheduler semaphores. Disjoint writers retain their
parallelism, and a shared domain is not silently converted into one global
mutex. External writers remain outside the scheduler; an immutable publication
barrier shields consumers without pretending those writers were serialized.

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

### Bound the whole run, not only its steps

Per-step budgets cannot bound a run: any number of individually-legal steps can
sum past any ceiling. `--run-timeout SECONDS` adds an outer wall budget for the
run itself.

```sh
safe-ci-dag-runner run --dag pipeline.yaml --run-timeout 900
```

On breach the scheduler stops launching, terminates every in-flight step's whole
process tree, marks those steps aborted with that reason, and **returns** — it
writes its profile rows and hands back a verdict rather than leaving the process
to be killed from outside, which would discard the evidence the bound exists to
capture. `SAFE_CI_DAG_RUNNER_RUN_TIMEOUT` sets the same budget for a wrapper that
cannot edit the command line.

The bounds are ordered, and the ordering is the point:

| bound | enforced by | on breach |
| --- | --- | --- |
| per-step wall / CPU | the runner (CPU budget needs a cgroup) | that step dies and is named |
| whole-run wall | the runner | in-flight steps cut, rows written, verdict returned |
| scope `RuntimeMaxSec` | systemd, when boxed | the whole scope dies |

Each level exists to stop the next one from firing. The scope budget is derived
automatically as the run budget plus the larger of 60 s and a tenth of it, and
the in-scope process reads the property back off the live unit rather than
trusting the request; a mismatch is an error unless `--allow-cgroup-failure`.

Because a step allowed to run as long as the whole run could only ever be
terminated by the outer bound — attributing the overrun to the run instead of to
the node that caused it — a run whose steps declare a wall budget at least as
large as `--run-timeout` is **refused before anything starts**, with the
offending steps named.

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
Without `--args`, the token is removed. `--stress N` duplicates the selected
graph at generation into `N` disconnected components with no edges between
copies. Each copy retains the original graph's internal dependency edges.
Named-resource scheduling is removed from the generated copies, so `-j`
controls how many copied steps run at once and may be set high for deliberate
core over-subscription. The report includes the exact pass ratio and the largest
number of step child processes measured alive at once. The modeled memory
footprint must still fit the box.

A singleton DAG can be generated on the fly; no `N`-node file is required:

```sh
printf '%s\n' '{"steps":[{"group":"stress","job":"singleton","cmd":"sleep 2"}]}' |
  safe-ci-dag-runner run --dag - --stress 100 --jobs 100 --no-profile
```

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

The ledger path is `SAFE_CI_CORE_LEDGER` when that variable is set. Otherwise it
is `$XDG_RUNTIME_DIR/safe-ci-dag-runner/core-reservations.json` when the runtime
directory exists, or
`<system-temporary-directory>/safe-ci-dag-runner-<uid>/core-reservations.json`.
Serialization uses the private sibling `core-reservations.json.lock`; both
commands use the same files. A crashed holder is identified by PID plus process
start time and reclaimed by the next ledger operation, so the state records live
ownership rather than a permanent allocation. Unsafe, foreign, non-regular, or
malformed state is refused instead of ignored.

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
