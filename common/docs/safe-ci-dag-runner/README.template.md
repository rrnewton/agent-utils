# safe-ci-dag-runner

`safe-ci-dag-runner` executes a directed acyclic graph of build and test steps
with dependency ordering, bounded concurrency, resource-aware planning, Linux
cgroup containment, and per-step profiling.

It provides:

- strict JSON and YAML DAG loading;
- deterministic list, visualization, conversion, and planning commands;
- memory- and resource-aware concurrent execution;
- nested cgroup-v2 containment with explicit unboxed opt-outs;
- profile feedback, parallel-speedup sweeps, and stress copies; and
- durable, collision-free CPU reservation for benchmark runs.

{{DISTRIBUTION}}

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
safe-ci-dag-runner ascii --dag pipeline.yaml
safe-ci-dag-runner run --dag pipeline.yaml
```

Containment is required by default. On a machine where cgroup-v2 delegation is
intentionally unavailable, add `--allow-cgroup-failure` to accept a visibly
unboxed run.

Useful discovery commands:

```sh
safe-ci-dag-runner quickstart
safe-ci-dag-runner --help
safe-ci-dag-runner --userguide
safe-ci-dag-runner capabilities
```

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

`safe-ci-dag-runner pin-run` provides the same hard reservation path from the
main command. `run --cores K` applies an exact cpuset only inside the runner's
own managed scope; it is incompatible with an unboxed opt-out. These commands
never fall back to an escapable process-affinity mask. All release live
reservations on exit and reclaim dead holders.

## License

MIT
