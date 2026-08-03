---
name: parallel-experiment-runner
description: Run N concurrent seed-sweep workers BOXED under safe-ci-dag-runner. A seed sweep is one command template with a {seed} placeholder run over a range of seeds — a chaos search, fuzz sweep, flaky-repro hunt, or parameter scan. Concurrency is a declared, enforced number (not unbounded fan-out); every worker runs under real cgroup CPU/memory/CPU-second/wall caps with a clean kill on breach; per-worker footprint is measured via a 1→2→4 calibration ramp; an up-front derived cost estimate (or honest UNSET) is printed before the sweep and measured actuals after. Use when running many parallel experiment instances that must not saturate the host, when a sweep needs real per-worker resource limits, or when hunting a seed that reproduces a bug. It reuses safe-ci-dag-runner's containment — it is not a second runner.
---

# parallel-experiment-runner

Runs a seed sweep as N boxed workers under `safe-ci-dag-runner`'s two-level cgroup-v2 scope.
Exists to prevent the failure where unbounded parallel experiments saturate the host and starve
their own measurements. Concurrency is declared+enforced; CPU-time (not wall) is the load-immune
guard; cost is estimated up front and measured after; a breach is a clean, named kill.

The CLI is the source of truth for usage — do not rely on this file for details. Run:

- `parallel-experiment-runner quickstart` — self-contained getting-started tour.
- `parallel-experiment-runner --help` — commands and flags.
- `parallel-experiment-runner --userguide` — the full user guide (complete reference).

Canonical commands:

- `parallel-experiment-runner run --name S --seeds 0-199 --cpu-cores 1 --memory 4G --cpu-timeout 120 --hit-regex 'panic|DIVERGENCE' --max-concurrency 32 --identity backend=ptrace image=demo5 -- hermit run --chaos --seed {seed} ./demo` — run a boxed sweep.
- `parallel-experiment-runner plan-round --seeds 0-99 --cpu-cores 2 --memory 8G --max-concurrency 40 -- ./workload {seed}` — resolve + print one round's enforced width and DAG (dry, no cgroups).
- `parallel-experiment-runner run --spec sweep.json` — drive the sweep from a JSON/YAML spec file.

Key rules: `--cpu-timeout` is a CPU-SECOND budget (omit = UNSET; never derive it from wall time).
Give at least one `--identity K=V` for reusable cost estimates (no identity ⇒ ephemeral, not
persisted). Boxing is on by default; the run refuses (exit 3) rather than run unboxed unless
`--allow-cgroup-failure` is given.
