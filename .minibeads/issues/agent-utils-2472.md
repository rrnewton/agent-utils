---
title: 'gate-abort-leaves-dagrun-scope: a gate stopped by SIGTERM records the run as aborted but leaves its dagrun scope and steps running outside admission'
status: open
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- gate
- dagrun
- validation
created_at: 2026-11-28T10:40:00-05:00
updated_at: 2026-11-28T10:40:00-05:00
---

# Description

`scripts/gate.py` documents its abort (module docstring, "An admitted run's command runs in its own process group"):
- SIGTERM, SIGINT or SIGHUP to gate.py forwards SIGTERM to the run's whole process group.
- It waits a grace period, SIGKILLs whatever is left, releases the slot and records the run as `aborted`.
- "so a stopped gate never leaves dagrun and its steps running outside admission".

On 2026-11-28 that promise did not hold.
- Gate run `1795879681459912579-b1731218` (a path-selected validate) was stopped with SIGTERM to its gate.py at 10:36:50 ET.
- gate.py printed "aborted by SIGTERM; run's process group stopped by SIGTERM". The ledger row says `aborted`, exit 143. The ticket left `gate queue`.
- The run's dagrun had re-homed itself into its own transient unit, `dagrun.slice/dagrun-3047529.scope`. The gate log names it: "run-boxed: pid 3047529 observed in .../dagrun-3047529.scope/supervisor". The unit stayed active/running after the abort. The dagrun supervisor (pid 3047529) was gone.
- About 30 processes were still running in the gate's worktree, in that scope's step cgroups:
  - `step-python.wrkslots.host-0`: `with-private-tmp --host-temp` -> `run_component_tests.py --component wrkslots --shard 0/3 -- -q -m ordinary_environment` and its pytest children;
  - `step-repository.check.wrkslots-debug-test`: `case_timings.py run-cargo -- cargo test ...`, the wrkslotsd test binary, `wrkslots.tests.idle_validation_host`, and `lsof +D` over fixture slots under the step's private temp root;
  - a `cargo test --release` (`case_timings.py run-cargo`) from another step, gone by the second look.
- They ran until the unit was stopped by name (`systemctl --user stop dagrun-3047529.scope`). After that, no process had a working directory in the worktree.
- The step journal (`journal.jsonl` in the run's gate-logs directory) ends with test events at 10:36:33. It has no step_end and no abort record from dagrun.

Effect:
- Steps of an aborted gate keep burning CPU outside admission and outside the gate's CPU cap, possibly for a long time (that step's timeout is hours).
- They hold the worktree, its cargo target and private temp roots.
- A requeued gate in the same worktree can race them.

Likely cause, to be confirmed in the fix:
- dagrun re-execs itself into `dagrun.slice/dagrun-<pid>.scope` (`DAGRUN_IN_SCOPE`, `rs/dagrun/src/cgroup.rs`). Its steps live in that scope's step cgroups, so they leave gate.py's process group and the gate's own scope.
- On SIGTERM, dagrun exited without the subtree kill it owns (`cgroup.kill` of the step cgroups; the code calls the normal-exit pass a backstop).
- gate.py's abort reaches only its process group, so nothing stopped the scope.

# Design

- The abort covers the whole run, wherever dagrun re-homed it.
  - dagrun, on SIGTERM/SIGINT/SIGHUP, kills every step cgroup subtree (`cgroup.kill`) and records the abort in its journal before exiting.
  - gate.py, after the grace period, also stops each dagrun scope the run reported. It reads them from the run's containment record ("run-boxed: pid N observed in .../dagrun-N.scope"), or from the journal's containment event. It stops that unit by name.
  - The same backstop runs if dagrun was SIGKILLed.
- The ledger's `aborted` record says whether any process remained after the abort, and how it was stopped.
- Tests:
  - a fake gate run whose command starts a dagrun scope with a long step; SIGTERM to gate.py leaves no process in the scope and the unit inactive;
  - the same with dagrun SIGKILLed first (the gate.py backstop alone).
- No wall-clock budget beyond the existing abort grace (owner rule 346).

# Acceptance Criteria

- After a gate is stopped by SIGTERM, SIGINT or SIGHUP, no process of its run remains, in its process group or in any dagrun scope it started, and the ledger says so.
- The two tests above pass in a gate.
- gate.py's abort docstring matches the behaviour.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28; number pre-assigned). Found while stopping an invalid gate: the gated worktree had been changed mid-run by an operator error, and the coordinator approved stopping and requeueing.

Related:
- #1254: the per-gate CPU containment that the leftover steps escaped.
- #1370: contained preflight; its notes record preflight dagrun scopes still running during a hold.
- #2065: the wrkslots steps that were left running.
