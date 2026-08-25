# dagrun

`dagrun` executes a directed acyclic graph of build and test steps with
dependency ordering, bounded concurrency, resource-aware planning, Linux cgroup
containment, and per-step profiling.

It provides:

- strict JSON and YAML DAG loading;
- deterministic list, visualization, conversion, and planning commands;
- independent active-step (`--max-steps`) and total CPU (`--max-cpus`) limits;
- memory- and named-resource-aware concurrent execution;
- nested cgroup-v2 containment with explicit unboxed opt-outs;
- profile feedback, parallel-speedup sweeps, and stress copies; and
- durable, collision-free CPU reservation for benchmark runs.

## Install

```sh
python3 -m pip install dagrun
```

Python 3.10 or newer is required. The installation provides the `dagrun` and
`cpuset-alloc` console commands, an importable package of the same name, and
typing metadata.

## Python API

The model, serializer, planner, scheduler, and visualization helpers are public:

```python
from dagrun import DagConfig, Step, to_ascii

dag = DagConfig(steps=(Step("build", "app", "compile", "make build"),))
print(to_ascii(dag))
```

`run_dag(..., jobs=N)` keeps a compatibility combined setting: `N` bounds active
steps and caps each runner-controlled step's width unless `core_budget` is supplied. New code
should call `run_dag_limited(..., max_steps=S, max_cpus=P)` when those limits
differ. The former `cpu_jobs=P` keyword remains a compatibility alias. Library
calls do not establish an outer cgroup, so `max_cpus=P` is a whole-run bandwidth
cap only when the caller supplies equivalent outer containment; it is never a
summed-width admission gate.

## Quick start

Save this as `pipeline.yaml`:

```yaml
steps:
  - group: build
    job: app
    desc: compile the application
    cmd: make build
  - group: test
    job: unit
    desc: run unit tests
    cmd: make test
    deps: [build.app]
```

Inspect and run it:

```sh
dagrun ascii --dag pipeline.yaml
dagrun run --dag pipeline.yaml
```

Containment is required by default. On a machine where cgroup-v2 delegation is
intentionally unavailable, add `--allow-cgroup-failure` to accept a visibly
unboxed run.

Useful discovery commands:

```sh
dagrun quickstart
dagrun --help
dagrun --userguide
dagrun capabilities
```

For example, `run -s2 -j200` permits at most two active DAG nodes while setting
the whole run's total CPU-bandwidth budget and each runner-controlled step's
maximum inner width to 200 core-equivalents. The two active steps may request
more than 200 workers in aggregate: the verified outer `cpu.max` makes them
share 200 core-equivalents instead of treating declared widths as reservations.
That quota is not an instantaneous thread-count or CPU-identity bound; use
`--cores K` for an exclusive fixed cpuset. Drop the `-s2` and the same command
permits **two hundred** active nodes: an absent `--max-steps` defaults to the
resolved `--max-cpus`, so a bandwidth number you type is also a concurrency
number unless you say otherwise. The long
spelling of `-j` is `--max-cpus`; migrate the 0.13 `run --jobs` spelling to it.
A hidden compatibility alias keeps existing 0.13 scripts working but is not
public run vocabulary; differing simultaneous values conflict and are rejected.
`sweep --jobs RANGE` remains the width-range option for a per-step speedup
experiment. A non-empty `jobs_flag` or `jobs_env` lets the runner rewrite an
inner width down to `--max-cpus`; `default_jobs_env` supplies the environment
channel inherited by steps, and `DAGRUN_JOBS_ENV` supplies that
default when the document omits it. Empty effective channels prevent rewriting.
When paired with a positive declared width, that width is self-managed and the
run refuses it if it exceeds the total budget.

The current planners do not jointly optimize inner width, co-running load, and
memory. Greedy-LPT and critical-path choose only dispatch order. CPA chooses
per-step widths from isolated speedup curves, but its no-overcommit makespan is
a planning reference; runtime overlap is still governed by `--max-steps` and
may oversubscribe the outer CPU budget.

## Attributable test-runner timeouts

A DAG node can contain a parallel test runner. On a node timeout, `dagrun`
first freezes durable test and process evidence, sends `SIGTERM` so the inner
runner can flush its state, waits a bounded grace, and only then escalates to
cgroup-wide/process-group `SIGKILL`.

Use three strictly nested bounds: the test runner's per-test timeout below the
DAG step timeout, and the step timeout below the whole-DAG timeout. The
innermost bound produces the clearest named failure. The DAG's process snapshot
is a backstop for a missing or mis-sized runner-native timeout, not a substitute
for one.

For a controlled harness, emit explicit boundaries as work starts and ends:

```text
##TEST-START suite::case
##TEST-END suite::case PASS
```

Several tests may be live concurrently. The timeout report lists the complete
live set and elapsed time for each; the longest-running is labelled only as the
*likely* culprit when more than one remains. Set
`DAGRUN_LOG_DIR` to retain the incrementally flushed per-step log
and `journal.jsonl` even if an outer supervisor later kills the runner.

For third-party runners, the pre-signal `/proc` snapshot reports CPU-burning
and wall-stalled processes as distinct signatures. It binds a process to a
test only when argv supplies a direct test identifier. A one-process-per-test
runner can therefore be bound; a shared-process runner cannot. Without a
direct identifier or recognized output boundaries, the report says
attribution is unavailable rather than guessing.

## CPU-set companion

The distribution also installs `cpuset-alloc`. It reserves disjoint cores in a
durable cross-process ledger and launches a command in a mutation-verified
`AllowedCPUs` cgroup scope. It refuses to run when hard tree-wide pinning cannot
be proved:

```sh
cpuset-alloc run --cores 2 --tag benchmark -- ./benchmark
cpuset-alloc status
cpuset-alloc reclaim
```

`dagrun pin-run` provides the same hard reservation path from the main
command. `run --cores K` applies an exact cpuset only inside the runner's own
managed scope; it is incompatible with an unboxed opt-out. These commands never
fall back to an escapable process-affinity mask. All release live reservations
on exit and reclaim dead holders.

## License

MIT
