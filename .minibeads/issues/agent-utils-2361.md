---
title: 'dagrun-run-cpu-budget-and-caller-delegated-nesting: dagrun gets a whole-run CPU-seconds budget, and its nesting under a cgroup delegated by the caller is documented, actionable and the same on both engines'
status: open
priority: 1
issue_type: feature
assignee: cl-shardcap
labels:
- dagrun
- cgroup
- validation
created_at: 2026-11-27T03:17:00-05:00
updated_at: 2026-11-27T03:17:00-05:00
---

# Description

A consuming harness wants to run `dagrun run` from a systemd user unit it starts itself, and have the run obey that unit's limits. The unit is a transient scope or service with `Delegate=yes` and its own `MemoryMax`. The harness also wants one bound on the run's total CPU time. A scratch probe on 2026-11-27 checked what dagrun does today, on both engines. The probe used no root and no daemon-reload, ran on a toy DAG, and cleaned up its units.

## Probe

**Setup:**
- Each run went in its own `systemd-run --user --scope --expand-environment=no -p Delegate=yes -p MemoryMax=2G` unit. A wrapper script set the route's environment and ran `common/bin/dagrun run --max-mem 1G --max-cpus 2 --max-steps 2 --run-timeout 12 --keep-going`.
- The toy DAG set `mem_cap_floor_bytes: 268435456` and had three steps, each with a 10 s wall budget:
  - `where`: printed its own cgroup and every ancestor's `memory.max`, `cpu.max` and `pids.max`, then slept 7 s.
  - `chain`: depended on `where` and slept 8 s, so the chain needed 15 s, past the 12 s run budget.
  - `alloc`: had a 64 MiB baseline hint and touched 700 MiB.
- dagrun refuses a `--run-timeout` that isn't longer than every step's own wall budget, so the outer bound can only fire across a chain. Without the floor setting, it refuses `--max-mem 1G` outright: the default floor is 8 GiB.

**Engines:** the Python engine ran with `DAGRUN_ENGINE=python`. The Rust engine was the default, a cached build of the same tree.

**Notation:** in the table, U is the unit's cgroup. "Unit cap" means the unit's 2 GiB `MemoryMax` applies to the run.

| Caller route | Engine | Where steps run | Unit cap | `--max-mem 1G` | `--max-cpus 2` | Per-step caps | `--run-timeout 12` | Exit |
|---|---|---|---|---|---|---|---|---|
| No dagrun cgroup environment | both | re-exec into `dagrun.slice/dagrun-PID.scope`, outside U | no | `memory.max` 1 GiB on the dagrun scope | `cpu.max` 2 CPUs there | yes: 20 MiB, 1 CPU; `alloc` killed at its 80 MiB cap | cut `chain` at 12 s | 1 |
| `DAGRUN_DIRECT_CGROUP=1` | Rust | inside U, at `U/dagrun-direct-PID/step-*` | yes, and the only memory bound | not enforced: no `memory.max` on the run cgroup | not enforced | not enforced: `alloc` touched 700 MiB and passed | cut `chain` at 12 s | 1 |
| `DAGRUN_DIRECT_CGROUP=1` | Python | value ignored: to this engine the variable holds an internal cgroup path, so the run re-execs as in the first row | no | 1 GiB on the dagrun scope | 2 CPUs | yes | cut at 12 s | 1 |
| `DAGRUN_DELEGATED_CGROUP=U`, caller moved into `U/supervisor`, `U/cgroup.subtree_control` empty | both | none: refused before any step, "could not establish inner per-step cgroups" (EACCES writing `memory.max` on its nested run cgroup) | - | - | - | - | - | 3 |
| Same, after the caller writes `+cpu +memory +pids` to `U/cgroup.subtree_control` | both | inside U, at `U/nested-run-PID-0/step-*`; the Rust scheduler sits in `U/nested-run-PID-0/supervisor` | yes, as an ancestor | `memory.max` 1 GiB on `U/nested-run-PID-0` | `cpu.max` 2 CPUs there | yes: 20 MiB, 1 CPU; `alloc` killed at 80 MiB (NO_RESULT) | cut `chain` at 12 s | 1 |

On every route, exit 1 came from the run-timeout abort.

**Whole-run CPU time:** no route and neither engine bounds a run's total CPU-seconds.
- `--run-timeout` is wall-clock time.
- A step's `cpu_timeout`, scaled by `--cpu-timeout-multiplier`, bounds only that step.
- `cpu.max` limits bandwidth (CPUs), not seconds. systemd has no CPU-seconds property either; `RuntimeMaxSec` is wall-clock.
- The only bound a caller can derive today is the product `--max-cpus × --run-timeout`, for example 2 × 12 = 24 CPU-seconds.

## Findings

1. **Nesting under a caller's unit works, but only through an undocumented contract.** The caller must:
   - run in a `Delegate=yes` unit;
   - create `U/supervisor` and move itself into it;
   - enable `+cpu +memory +pids` in `U/cgroup.subtree_control`;
   - export `DAGRUN_DELEGATED_CGROUP=U`.

   Only the outer dagrun's own delegated steps do all of this today. Skip the subtree_control step, and the refusal reads like a permission problem (EACCES) rather than naming the missing controllers.
2. **Without the delegated route, the run leaves the caller's unit.** The default top-level run re-execs into `dagrun.slice`, so the caller's `MemoryMax` and kill boundary don't cover it.
3. **`DAGRUN_DIRECT_CGROUP` means different things on the two engines.**
   - Rust treats `=1` as an opt-in route: teardown only, no caps, run kept inside the current cgroup.
   - Python uses the same name internally for a path and silently ignores `=1`.
4. **There is no whole-run CPU-seconds budget.**

# Design

- **Whole-run CPU budget.** Add `--run-cpu-budget SECONDS` (and `DAGRUN_RUN_CPU_BUDGET`) on both engines.
  - The runner reads `usage_usec` from `cpu.stat` on the run's own aggregate cgroup: the nested run root when delegated, the run scope otherwise. When the total reaches the budget, it cuts in-flight steps exactly like `--run-timeout`: still reporting, with a `RUN CPU BUDGET` line naming the steps it cut.
  - Without cgroups, it uses the procfs process-group floor and says so, as the per-step CPU budget already does.
  - The final summary line reports the run's CPU-seconds whether or not a budget is set.
  - A delegated run inherits a finite parent budget as its default and upper bound, the way `--max-mem` already does.
- **Caller-delegated nesting.**
  - When `DAGRUN_DELEGATED_CGROUP` names a valid delegation whose `cgroup.subtree_control` lacks a controller dagrun needs, dagrun enables it itself if it may. That's legal once the caller sits in `supervisor`. If it may not, it refuses with the missing controller names and the exact command to run.
  - A documented recipe, also in `dagrun run --help`, covers a caller starting from a `Delegate=yes` unit:
    - the supervisor move;
    - the controllers;
    - the environment variable;
    - which bounds apply (the table above).
- **One meaning for `DAGRUN_DIRECT_CGROUP`.** Both engines either honour `=1` with the same documented semantics, or refuse a value that isn't a path, instead of silently re-execing. The internal path variable moves to a name of its own.

# Acceptance Criteria

- Both engines, in a `Delegate=yes` user unit with `MemoryMax`, given the documented recipe:
  - keep every step inside the unit;
  - enforce `--max-mem` and `--max-cpus` on the nested run root, plus each step's own caps;
  - cut a CPU-burning chain at `--run-cpu-budget`, with the `RUN CPU BUDGET` line naming the steps.

  Integration tests prove this on both engines; they skip with a stated reason where user units can't be delegated.
- A delegated run whose unit lacks controllers either enables them and runs, or refuses naming the missing controllers. It never fails with a bare EACCES.
- The run summary reports total CPU-seconds.
- A parity test pins one behaviour for `DAGRUN_DIRECT_CGROUP=1` on both engines.
- `dagrun run --help` and the dagrun docs carry the caller recipe and the per-route bounds table.

# Notes

- Related:
  - #29 delegated-scope-smokes keeps top-level boxing covered outside delegated validation.
  - #2207 made a no-signal learned-cap OOM a NO_RESULT, which is what the `alloc` step shows on the boxed routes.
  - #1763 is the learned-cap rule.
- The probe's scratch units and directories were removed after the run.
