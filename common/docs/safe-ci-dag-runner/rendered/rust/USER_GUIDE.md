# safe-ci-dag-runner user guide

`safe-ci-dag-runner` models build and test work as a DAG, plans it
deterministically, and executes ready steps concurrently. Dependencies, memory
budgets, named resource caps, timeouts, CPU reservations, and Linux containment
all constrain execution without changing the graph's meaning.

## Installation and library use

```sh
cargo install safe-ci-dag-runner
```

Rust 1.85 or newer is required. The crate installs both binaries. For library
use, declare the dependency and import the crate as `safe_ci_dag_runner`:

```toml
[dependencies]
safe-ci-dag-runner = "0.15"
```

```rust
use safe_ci_dag_runner::{dag_from_yaml, to_ascii};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dag = dag_from_yaml("steps: []")?;
    println!("{}", to_ascii(&dag, None));
    Ok(())
}
```

Public modules cover containment, CPU reservations, resource sizing, profile
summaries, deterministic planning, scheduling, serialization, and
visualization. The `run_dag` function is the explicit uncontained scheduler;
`run_dag_boxed` applies a caller-supplied cgroup manager when present. The
console command establishes and verifies the outer systemd scope. Enforced
containment requires Linux with cgroup v2 and a delegated systemd user scope.
`run_dag(..., combined_limit)` and `run_dag_boxed(..., combined_limit)` treat
that argument as a compatibility combined active-step and per-step-width limit.
Use `run_dag_limited(..., max_steps, max_cpus, ...)` or a boxed limited variant
to choose the two values independently. Without an externally established outer
quota, these helpers do not cap aggregate CPU bandwidth or serialize overlapping
widths. `cap_config_max_cpus` applies the same per-step cap to runner-controlled
commands without starting a run; it leaves
self-managed fixed widths unchanged so the run helpers can refuse an
over-budget command truthfully.
The low-level `allocate_widths(...)` helper returns
`Result<HashMap<_, _>, InfeasibleAllocationError>`; 0.15 makes an over-budget
self-managed fixed command an explicit error rather than an executable-looking
width map.

## A first DAG

Each step is identified by a `group.job` tag. `deps` names predecessor tags.
The optional `hint` object supplies estimates and limits; top-level
`resource_caps` limits caller-defined scarce resources.

Every resource a step demands must have a cap declared. An UNDECLARED
resource is refused before any node starts, because it is not the same thing
as a cap of `0`: undeclared means capacity you forgot to grant, while `0`
means the step is blocked on purpose. Both would otherwise leave the step
permanently unready with nothing said, so declare the capacity — or write the
cap as `0` to say the block is deliberate.

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
safe-ci-dag-runner run --dag pipeline.yaml --max-steps 2 --max-cpus 8
safe-ci-dag-runner run --dag pipeline.yaml --max-mem 8G
safe-ci-dag-runner run --dag pipeline.yaml --show-plan --profile
```

`-s` / `--max-steps` bounds how many DAG nodes may be active. `-j` /
`--max-cpus` independently sets the **total CPU capacity for the whole run**, in
core-equivalents, and caps any one runner-controlled step at that width. It does
not reserve or subtract declared widths as steps start. Thus `-s2 -j8` may run
two `-j8` steps together: their sixteen workers contend inside the outer
eight-core-equivalent quota. This oversubscription can help when work stalls or
parallel phases do not align, and can hurt when both steps are CPU-bound. The default CPU total is
the effective container/affinity capacity tightened by the shared aggregate
slice's 90% host budget. `--max-steps` defaults to that value; an undeclared
step separately keeps `default_step_cpu_count` (one in the CLI defaults).

A non-empty effective `jobs_flag` makes the inner width runner-controlled: when
an authored or profile-derived width exceeds `--max-cpus`, the planner and
scheduler cap the recommendation, appended command flag, and per-step `cpu.max`
together. An empty or whitespace-only `jobs_flag` instead
prevents command rewriting; paired with a positive `preferred_inner_jobs`, it
declares a self-managed fixed command width. If that declared width
exceeds the run budget, the run is refused before any DAG step process is
created because silently throttling (for example) a hardcoded `make -j32`
inside `--max-cpus 16` would oversubscribe and mislabel its memory/profile data.
File-backed runs reject before cgroup setup; a stdin DAG may already have
entered its outer scope before it can be read and validated. A sweep likewise requires a non-empty
`jobs_flag`, since otherwise changing `sweep --jobs` would not change the guest.

The runner cannot infer hidden concurrency that a command does not declare. An
arbitrary guest may still create more threads than `--max-cpus`; outer
`cpu.max` limits their total CPU bandwidth, not their count. Use a controllable
`jobs_flag`, fix the command's own worker setting, or use `--cores` when fixed
CPU eligibility is required.

`--max-mem` derives a conservative, model-based `--max-steps` ceiling from the worst-case
footprint at each step's applied inner width. If an explicit step ceiling is
also present, the tighter value wins. Hard caps, learned/authored RSS, runtime
defaults, the outer safety factor, and selected `engine_only` steps all count;
intentional skips do not. If even one runnable step or the configured footprint
floor exceeds the budget, the run refuses instead of claiming one step fits.
CPA reports the same state as `infeasible-memory`. Named resources act as semaphores in
addition to the step and memory limits. A failed step prevents new dependent work; `--keep-going` lets
already-running work reach a verdict.

Under boxing, the run scope also receives `CPUQuota=<max-cpus>*100%`, and the
live `cpu.max` value is read back before work starts. This makes `--max-cpus N`
an N-core-equivalent **CPU-bandwidth** ceiling as well as the scheduler's width
ceiling for any one step. It is not an instantaneous thread-count or CPU-identity bound: CFS quota
may briefly run more than N runnable tasks on more than N CPUs and throttle them
later in the quota period. An unpinned run may also migrate from CPUs A/B to C/D
without exceeding its long-window budget. Use `--cores K` when exact eligible
CPU identities are required. The runner does not use `cpu.weight` as a cap.

Migration is deliberately explicit. Before 0.13, `run -j N` meant maximum
active steps; migrate that old intent to `run -s N`, or use `-s N -j N` when N
should bound both dimensions. In 0.13, the total-CPU long option was
`run --jobs N`; replace it with `run --max-cpus N`. The `-j N` shorthand keeps
its 0.13 total-CPU meaning. A hidden `run --jobs N` compatibility alias remains
temporarily so existing 0.13 scripts do not break, but it is omitted from help
and should not be combined with `--max-cpus`; differing simultaneous values are
rejected. New commands should use `--max-cpus`; the public
`sweep --jobs RANGE` spelling remains the option for inner widths being measured.
In 0.15, legal per-step widths no longer consume additive scheduler tokens:
`--max-steps` governs overlap and several steps may request more than
`--max-cpus` in aggregate while the boxed outer quota arbitrates their shared
bandwidth. Library callers that relied on 0.14's width-sum serialization should
use `max_steps`, named resources, or their own admission policy explicitly.

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
Named-resource scheduling is removed from the generated copies, so
`--max-steps` controls how many copied nodes may be active while `--max-cpus`
caps each copy's requested width and their shared outer CPU bandwidth. The report includes the exact pass ratio
and the largest number of step child processes measured alive at once. The
modeled memory footprint must still fit the box. Expansion is also refused when
it would create more than 100,000 generated DAG nodes/control units, so a tiny
guest memory hint cannot turn `--stress` into an unbounded host-side allocation.

A singleton DAG can be generated on the fly; no `N`-node file is required:

```sh
printf '%s\n' '{"steps":[{"group":"stress","job":"singleton","cmd":"sleep 2"}]}' |
  safe-ci-dag-runner run --dag - --stress 100 --max-steps 100 --max-cpus 100 --no-profile
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

No central daemon is required for the current fixed-slice mode: every allocator
serializes through the shared durable ledger and claims an explicit set of CPU
IDs. A future service could provide dynamic or fair machine-wide allocation,
but ordinary `--max-cpus` deliberately remains a shared bandwidth and per-step
width limit rather than pretending to hand out exclusive moving CPU slices.

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
