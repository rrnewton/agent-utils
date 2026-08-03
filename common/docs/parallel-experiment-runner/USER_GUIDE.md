# parallel-experiment-runner — user guide

Run **N concurrent seed-sweep workers, BOXED** under `safe-ci-dag-runner`'s two-level
cgroup-v2 scope. A seed sweep is one command template with a `{seed}` placeholder, run over a
range of seeds — a chaos search, a fuzz sweep, a flaky-repro hunt, a parameter scan.

This tool exists because unbounded parallel experiments once saturated a host to ~470
concurrent processes, starving the very measurements they were launched to produce. It fixes
that structurally: **concurrency is a declared, enforced number**, every worker runs under real
cgroup limits, and a worker that breaches a limit is cleanly killed with a message naming what
breached and by how much.

It is a thin, additive generalization of `safe-ci-dag-runner`: each seed becomes one boxed
`Step`, and the existing scheduler runs them. There is **no second runner** — the containment,
teardown, CPU-second budget, and per-step measurement are all the CI runner's, reused.

## The four guarantees

1. **CPU-time budgets, not wall.** `--cpu-timeout N` is a per-worker **CPU-second** budget
   (user+system), measured from the worker's cgroup `cpu.stat` and immune to machine load. It is
   never derived from wall time: on an N-core host, N wall-seconds is up to N CPU-seconds, so a
   wall-derived CPU budget would be ~1/N of what a worker needs and would false-kill healthy
   workers in a way indistinguishable from a flaky test. Omit the flag to leave it **UNSET**
   (honest; the load-tolerant wall backstop still applies) rather than guess a too-tight number.
2. **Declared + enforced concurrency.** The width is resolved from three budgets — the
   coordinator's lane, live host capacity, and the measured per-worker footprint — and the round
   runs *exactly* that many workers. Never "however many the caller spawned".
3. **Estimate up front, actuals on completion.** Before the sweep, a **derived** per-worker cost
   estimate (from prior samples of the same profile key) is printed — or an explicit **UNSET**
   when there is no comparable sample. Never a plausible-looking constant. After each round and at
   the end, the measured wall and CPU actuals are printed.
4. **Clean kill naming the breach.** A worker over its CPU budget, over `memory.max` (OOM), or
   past its wall backstop is reaped via `cgroup.kill` (setsid-proof) and reported as e.g.
   `CPU-TIMEOUT: used 3.0s cpu >= budget 3s`.

## Quick start

```bash
parallel-experiment-runner run \
  --name chaos-divergence --seeds 0-199 \
  --cpu-cores 1 --memory 4G --cpu-timeout 120 --wall-timeout 900 \
  --hit-regex 'DIVERGENCE|panic' --max-concurrency 32 \
  --identity backend=ptrace image=demo5 \
  -- target-runner --chaos --seed {seed} ./demo
```

* Every occurrence of `{seed}` in the argv after `--` is replaced with the concrete seed.
* Per-worker logs are written to `<work-dir>/ignored/logs/seed-<n>.log`; the run prints only an
  aggregate summary, so a wide round does not interleave hundreds of streams.
* The runner ramps width `1 → 2 → 4 → …`, measuring the real per-worker footprint at width 1
  before scaling, and downshifts immediately if the lane shrinks or the host gets busy.

`plan-round` shows the resolved width and the lowered DAG without running anything (and needs no
cgroups) — use it to check how wide a round *would* go:

```bash
parallel-experiment-runner plan-round --name demo --seeds 0-99 \
  --cpu-cores 2 --memory 8G --max-concurrency 40 -- ./workload {seed}
```

## Hits, misses, and breaches

Each worker ends in exactly one status. **Breaches are classified first**, so a worker the runner
had to kill is never miscounted as a discovered hit:

| status | meaning |
| --- | --- |
| `hit` | the hit regex matched the log **or** the exit code was a declared hit code |
| `miss` | ran cleanly (exit 0) without the target condition |
| `command-error` | nonzero exit that is not a declared hit code |
| `cpu-timeout` | exceeded the CPU-second budget → reaped |
| `memory-cap` | hit `memory.max` → OOM-killed inside its own cgroup |
| `timeout` | exceeded the wall backstop (a hang burning no CPU) |
| `cancelled` | never ran / eager-cancelled |

A hunt for a bug sets `--hit-regex` (e.g. `panic|DIVERGENCE|ASAN`) and/or `--hit-exit-codes`
(e.g. `134` for SIGABRT). With neither, the default hit is exit code `0` (a sweep for successful
runs). The run exits nonzero if any worker breached a limit, so a wrapper notices.

## Profile keys and estimates

Samples are grouped by a **stable profile key** so estimates are apples-to-apples. The key hashes
the command template (with the seed removed), the `--identity` fields, and the hit semantics. Two
runs that differ on a mandatory identity dimension (`backend`, `kernel_id`, `image_id`, `vcpu`,
`guest_memory`, `accelerator`) never share history, even under a manual `--profile-key`.

With **no** `--identity` and no `--profile-key`, the key is *ephemeral*: the sweep still runs and
calibrates, but its samples are **not** persisted, so an unattributable run cannot contaminate
another sweep's estimates. Give at least one `--identity K=V` to build reusable history.

The estimate is conservative where it matters: median wall (wall is contention-inflated, so its
median is the fairer central figure) and p90 CPU/memory (don't under-provision).

## Boxing

Boxing is **on by default**. On first launch the process re-execs into a transient
`systemd-run --user --scope` (a delegated cgroup) named under `parallel-experiment.slice`, then
carves a child cgroup per worker with its `memory.max` / `cpu.max` caps. If cgroup-v2 + a working
`systemd --user` scope are unavailable, the run refuses with exit 3 rather than silently run
unboxed. `--allow-cgroup-failure` opts into an explicit unboxed run (process-group teardown only,
no per-worker caps) — not recommended, and the reason the box is the point.

## Spec files

Instead of inline flags, pass `--spec sweep.json` (or `.yaml`, needing PyYAML):

```json
{
  "name": "chaos-divergence",
  "command": ["target-runner", "--chaos", "--seed", "{seed}", "./demo"],
  "worker_limits": {"cpu_cores": 1, "memory_bytes": 4294967296, "cpu_timeout_s": 120, "wall_timeout_s": 900},
  "hit": {"regex": "DIVERGENCE|panic", "hit_exit_codes": [134]},
  "identity": {"backend": "ptrace", "image": "demo5"}
}
```

## Flags

Run `parallel-experiment-runner run --help` for the full list. The important ones:

| flag | meaning |
| --- | --- |
| `--seeds` | seed spec: `0-99,200,300-305` (inclusive ranges + singletons) |
| `--cpu-cores` | per-worker CPU cores → inner `cpu.max` and the width core-unit |
| `--memory` | per-worker memory cap (e.g. `4G`) → inner `memory.max` |
| `--cpu-timeout` | per-worker CPU-second budget; omit = UNSET (never wall-derived) |
| `--wall-timeout` | per-worker wall backstop seconds (hang guard only) |
| `--disk` | per-worker disk reserve used to bound width (cgroup-v2 has no space controller) |
| `--hit-regex` / `--hit-exit-codes` | what marks a worker a HIT |
| `--identity K=V` | apples-to-apples fields hashed into the profile key |
| `--max-concurrency` | hard ceiling on concurrent workers |
| `--slice-cpu/-memory/-disk` | the coordinator lane envelope (default: whole machine) |
| `--work-dir` / `--log-dir` / `--profile-store` | where logs + samples live |
| `--format` | `human` (default) or `json` |
