# Parallel Experiment Runner Design

**Date:** 2026-07-29  
**Status:** proposed; implementation waits for owner review  
**Target:** `agent-utils` v0.12.0 (additive minor release)  
**Proposed tool:** `parallel-experiment-runner`

## 1. Problem and scope

Hermit chaos experiments search a seed space by running many independent guest
executions. A single Hermit container remains sequential internally, but a host
with 158 physical cores can explore many seeds concurrently. Existing sweeps
have been ad hoc and sequential; some also streamed enough trace/build output to
overwhelm agents. Simply launching dozens of QEMU or Hermit processes is unsafe:
memory and disk footprints vary, host load changes, and an interrupted process
can leave descendants behind.

The proposed runner turns a seed search into a sequence of dynamically generated
`safe-ci-dag-runner` rounds. It does not introduce a second scheduler or a
second isolation/reaping implementation. `safe-ci-dag-runner` remains the
executor and supplies its existing two-level cgroup containment, per-step
profiling, process-group capture, eager teardown, and profile store.

The first release targets Linux/cgroup-v2 and Hermit `--chaos`/QEMU-style
workers, while keeping the experiment API generic enough for backend builds,
CI shards, fuzzers, and other independent parameter sweeps.

## 2. Today: a fixed DAG and per-step profiles

Today `safe-ci-dag-runner` consumes a caller-authored JSON or YAML `DagConfig`.
The file normally lives in the repository and contains a stable set of steps,
dependency edges, commands, resource hints, and scarce-resource caps. The
runner:

- selects a memory-aware concurrency;
- executes each step inside a child cgroup beneath a delegated outer scope;
- applies inner CPU and memory caps;
- captures per-step wall time, peak memory, CPU usage, pressure, throttling,
  and ambient-load context; and
- feeds bounded historical samples back into later plans.

That model assumes step identity and graph shape are known before invocation.
A seed search is different: the coordinator creates a fresh set of seed steps
for each round, stops creating rounds after enough hits, and may receive a new
resource allocation between rounds.

## 3. Proposed model: dynamically generated DAGs per round

`parallel-experiment-runner` is an outer-loop planner over the existing DAG
executor:

```text
ExperimentSpec + seed cursor + coordinator ResourceSlice + profile history
                              |
                              v
                    generate_round_dag(...)
                              |
                              v
                 safe_ci_dag_runner.run_dag(DagConfig)
                              |
                              v
              RoundResult -> hits, profiles, next width
```

Each round creates an in-memory `DagConfig`; no generated DAG must be committed
or left on disk. A round contains one independent step per seed. Each step has:

- a unique execution tag such as `seed.00000042`;
- a command produced from an argument-vector template containing `{seed}`;
- the shared stable profile key described below;
- per-worker CPU, memory, PID, timeout, and disk/log limits; and
- a machine-readable result path used to classify hit/miss/error without
  streaming the worker log.

The generated graph is intentionally shallow: seed steps have no dependencies
and share resource constraints. Dependencies may be added later for a common
image-preparation step, but v0.12.0 will require preparation to be complete
before the sweep so it is not repeated in every round.

### 3.1 Public Python API

The additive API should be usable both from the CLI and a long-running
coordinator:

```python
spec = ExperimentSpec(
    name="btrfs-chunk-recover",
    command=("hermit", "run", "--chaos", "--seed", "{seed}", "--", "./case"),
    profile_key=ProfileKey(...),
    worker_limits=WorkerLimits(cpu_cores=1, memory_bytes=4 << 30, disk_bytes=8 << 30),
    hit=HitCondition(regex="BUG:|panic|Segmentation fault"),
)

runner = ParallelExperimentRunner(profile_store=store)
plan = runner.plan_round(spec, seeds, resource_slice)
result = runner.execute_round(plan)       # delegates to safe-ci-dag-runner
next_plan = runner.plan_next(result, resource_slice)
```

Core types:

- `ExperimentSpec`: immutable experiment identity, command factory/template,
  worker hard limits, timeout, and hit predicate.
- `ResourceSlice`: the coordinator-assigned CPU/memory/disk envelope and
  revision.
- `RoundPlan`: selected seeds, calibrated width, generated `DagConfig`, and the
  evidence behind the width decision.
- `RoundResult`: per-seed structured outcomes, hits, throughput, observed
  footprint, and teardown/accounting status.

The CLI is a thin adapter:

```text
parallel-experiment-runner run \
  --spec experiment.yaml \
  --allocation resource-slices.json --lane btrfs-sweep \
  --seed-start 1 --seed-count 10000
```

`plan-round --format json` will expose the generated plan without running it.
This is the dynamic equivalent of inspecting a committed DAG. A debugging
option may emit the generated canonical DAG, but that output is diagnostic, not
the source of truth.

### 3.2 Why an on-the-fly executor is not a second executor

The outer loop decides *which steps exist in the next round*. It must not run
subprocesses itself. `execute_round` calls the same `safe-ci-dag-runner`
scheduler used for fixed DAGs, with the same cgroup manager, metrics sink,
failure semantics, and reaper. This keeps one implementation of containment and
prevents behavioral drift between CI and experiment workloads.

## 4. Stable profile keys

Generated step tags include seeds and therefore cannot be historical profile
keys. The DAG/profile API needs an optional `profile_key` independent of
`Step.tag`. All seed steps in an apples-to-apples experiment round share that
key, allowing later rounds and later invocations to reuse CPU, memory, disk, and
wall-time history.

### 4.1 Canonical key inputs

For QEMU/Hermit runs, an automatic key should hash a canonical identity record:

- tool and profile-schema versions;
- Hermit executable content SHA-256 and backend;
- Hermit determinism/chaos flags other than the seed;
- guest command template with the seed replaced by `{seed}`;
- VM kernel, root image, snapshot, and target-artifact content IDs;
- vCPU count, guest-memory size, disk/overlay class, and accelerator;
- host class facts that materially change execution (architecture and PMU/KVM
  mode, not hostname); and
- workload input IDs and hit-condition semantics.

The key deliberately excludes the seed, run ID, timestamps, temporary paths,
log paths, and coordinator allocation revision.

### 4.2 Apples-to-apples caveat and agent judgment

Profile reuse is safe only when samples describe the same execution shape.
Two commands that look textually similar may not be comparable: a different
kernel or qcow image, KVM versus TCG, changed guest memory/vCPU count, cold
versus prewarmed caches, a different btrfs image, or a changed target binary can
move wall time and peak memory substantially.

Therefore stable-key selection is an explicit correctness decision:

1. The runner prints the canonical identity and key before calibration.
2. An agent may provide a human-readable `--profile-key`, but must record why
   the grouped runs are apples-to-apples.
3. If a content identity cannot be established, the runner uses an ephemeral
   per-invocation key and does **not** contaminate shared history.
4. A manual key never overrides incompatible hard dimensions (backend,
   vCPU/memory class, image/kernel ID); those remain mandatory suffixes.
5. Every summary records the key and identity fields so a reviewer can audit
   reuse.

Historical samples use the existing bounded/mergeable profile-store machinery.
Planning should use conservative high-percentile memory/disk estimates and a
contention-discounted wall-time estimate rather than a raw mean.

## 5. Coordinator resource carving

The coordinator, not an individual runner, decides how the 158-core machine is
split among btrfs seed sweeps, backend builds, and CI. A reloadable allocation
file is the contract:

```json
{
  "revision": 12,
  "host_reserve": {"cpu_cores": 8, "memory_bytes": "64G", "disk_bytes": "200G"},
  "lanes": {
    "btrfs-sweep":   {"cpu_cores": 96, "memory_bytes": "384G", "disk_bytes": "800G"},
    "backend-builds": {"cpu_cores": 40, "memory_bytes": "192G", "disk_bytes": "400G"},
    "ci":             {"cpu_cores": 14, "memory_bytes": "96G",  "disk_bytes": "200G"}
  }
}
```

The coordinator writes a new revision atomically when priorities change. The
runner reloads its lane before every round. A reduction takes effect before new
workers launch; active workers remain within their already enforced caps and
finish or are cancelled as a group. An increase does not cause an immediate
jump: it only permits the next calibration doubling.

The runner also samples physical cores, load average/CPU pressure, cgroup and
host `MemAvailable`, and filesystem free space. Live capacity can reduce a lane
allocation but never increase it. The decision is:

```text
usable = min(coordinator lane, live host capacity minus reserves)
width  = min(cpu slots, memory slots, disk slots, configured ceiling)
```

Every decision reports the independent slot counts so the limiting resource is
visible.

## 6. Mandatory calibration ramp: 1 -> 2 -> 4 -> ...

No invocation may jump from zero knowledge to dozens of workers.

1. **Profile one.** Run one real seed sequentially. Record cgroup peak memory,
   CPU seconds/effective cores, wall time, log/work-disk growth, PID peak,
   OOM/timeout status, and ambient load.
2. **Validate two.** Compute conservative per-instance upper bounds (measured
   high water plus headroom, bounded by declared hard caps). Launch two only if
   `upper_bound × 2` fits every coordinator and live-host dimension.
3. **Double.** Repeat at 4, 8, 16, and so on. A stage must complete and publish
   measurements before the next doubling.
4. **Steady state.** Use the last successful power-of-two width for subsequent
   rounds. Reload the coordinator allocation and live probes before each round.
5. **Downshift immediately.** If load, memory, disk, or allocation shrinks,
   reduce the next round to the largest safe lower width. Never rely on a past
   wide run to ignore current pressure.
6. **Restart calibration when identity changes.** A new stable profile key,
   worker cap, VM image, backend, or execution class restarts at one. Historical
   data may inform the initial hard caps but may not skip the ramp.

The ramp uses real search seeds, so calibration work is not discarded. Hits
found during calibration are reported normally. A hit can stop future rounds,
but the current round is reaped through the normal scheduler path.

## 7. Resource containment, caps, logs, and teardown

**Framing (v0.2.0):** the threat model is a **bug in our own code**, not an
adversary. We trust the workload's intent and distrust exactly one thing about
it — its **resource usage**. This is therefore **resource containment**, a
*resource box*, **not a security sandbox**: it deliberately does **not** reach
for seccomp or user-namespace isolation. Containment is enforced on **four
independent axes**, each mapped to one named failure mode:

- **cpu** — 'run forever': a CPU-**second** budget from `cpu.stat` (never wall).
- **memory** — 'leak memory': `memory.max`, OOM via `memory.events`.
- **pids** — 'fork bomb': `pids.max`. Neither `cpu.max` nor `memory.max` stops
  PID exhaustion; only a pids cap does. A breach **contains** the fork (`EAGAIN`
  from `clone`/`fork`) rather than kernel-killing the worker, so the denied-fork
  count (`pids.events` `max` line) is captured and the cpu/wall guard reaps the
  contained worker. The count rides an **in-memory `StepOutcome` field, not a new
  profile-store CSV column**, so the `cross/` differential schema stays
  byte-identical and the in-flight Rust-runner workstream is not disturbed.
- **wall** — defence-in-depth backstop only, **derived at ~3× the CPU budget**
  when left unset (never a hardcoded default when a CPU budget exists).

The kill message names **what breached and by how much** (e.g. `PIDS-CAP: 4
fork/clone(s) denied at pids.max 16`). Because a contained fork bomb is reaped by
the wall/CPU guard, the classifier ranks `pids-cap` ahead of `timeout` using the
in-memory denied-fork count, so a fork bomb is never mislabelled as a plain hang.

This design builds directly on `safe-ci-dag-runner`'s existing two levels:

1. **Outer delegated scope:** one transient systemd/cgroup-v2 scope for the
   experiment lane invocation, capped to the coordinator's CPU and memory
   allocation with swap disabled.
2. **Per-seed child cgroup:** CPU, memory, and PID caps for each generated DAG
   step, plus authoritative `memory.peak`, `memory.events`, `cpu.stat`, pressure,
   and thread/PID measurements.

The existing scheduler launches each step with `start_new_session=True`,
captures that worker's PGID, kills its child cgroup first, then uses `killpg`
only on the captured PGID. The outer-scope teardown is the final backstop for
`setsid`/double-fork escapees. The experiment runner must call these APIs; it
must never use broad `pkill`, process-name matching, or host-wide cleanup.

Disk has no generic cgroup-v2 space controller. The design therefore combines:

- a coordinator lane disk budget and host free-space reserve;
- one preallocated/capped per-worker workspace or qcow overlay;
- an `RLIMIT_FSIZE`/bounded-log cap for ordinary worker output;
- dynamic free-space checks before every round; and
- immediate stop-before-launch when the reserve would be crossed.

Each worker writes combined stdout/stderr directly to
`ignored/logs/parallel-experiment-runner/<run-id>/seed-<N>.log`. Nothing streams
to the agent. The console receives only ramp decisions, aggregate counts,
throughput, hit seeds, and bounded log paths. A machine-readable summary JSON
contains the full structured results without embedding raw logs.

## 8. Result and failure semantics

Worker outcomes distinguish `miss`, `hit`, `command-error`, `cpu-timeout`,
`timeout`, `memory-cap`, `pids-cap`, `disk-cap`, and `cancelled`. A hit predicate
may be a bounded log regex, a designated exit code, or a structured result-file
field. Infrastructure failures are never silently counted as target hits.
Breaches are classified before hits, and `pids-cap` outranks `timeout` (see §7),
so a contained fork bomb reaped by the wall guard is named as the fork bomb it is.

The final summary includes:

- agent-utils/tool/schema versions and stable profile key;
- coordinator allocation revisions observed;
- every calibration stage and limiting dimension;
- seeds attempted and hit seeds;
- failures, timeouts, OOM/disk-cap events, and reaped leftovers;
- wall time, seeds/second, and peak concurrent workers; and
- the log directory and structured summary path.

## 9. Versioning and compatibility

This is additive and should release as **agent-utils v0.12.0**:

- new Python-first `parallel-experiment-runner` tool, shipped `0.1.0`, now at
  tool version `0.2.0` (additive four-axis containment: the `pids` axis and the
  derived wall backstop);
- additive dynamic-DAG/profile-key API in `safe-ci-dag-runner`;
- the `pids` axis rides an in-memory `StepOutcome.pids_events` field and adds no
  profile-store CSV column, so the `cross/` schema and the Rust runner are
  unaffected; live cgroup boxing (`cpu.*` columns) remains out of differential
  scope, as before;
- no change to existing fixed JSON/YAML DAG behavior or schemas when
  `profile_key` is absent; and
- a future Rust port/differential test tracked explicitly rather than delaying
  the first safe coordinator integration.

Profile records gain a schema version and optional stable key. Existing rows
without the key remain readable and retain their current tag-based identity.

## 10. Implementation sequence after review

1. Add `profile_key` to the safe-ci model/profile store with compatibility and
   deterministic tests.
2. Add the dynamic round-planning types and pure calibration logic.
3. Build the Python CLI as an outer loop that generates `DagConfig` values and
   calls the existing scheduler/cgroup APIs.
4. Add allocation-file reload and coordinator-facing `plan-round` JSON.
5. Add synthetic tests for stable keys, mandatory doubling, downshift, cap
   rejection, hit semantics, and no-broad-kill teardown.
6. Run one small detached sweep through the real cgroup executor and report
   1/2/4 stages, caps, zero leftovers, and throughput.
7. Bump/package/tag v0.12.0, land through review, then integrate a real btrfs
   seed sweep.

Implementation and the semver bump are intentionally gated on owner review of
this document.
