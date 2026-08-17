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

`run_dag(..., jobs=N)` treats `N` as a compatibility combined active-step and
total-CPU limit unless `core_budget` is supplied. Use
`run_dag_limited(..., max_steps=S, max_cpus=P)` for explicit independent
limits; the former `cpu_jobs=P` keyword remains a compatibility alias. A
pre-0.13 outer-fan-out-only library caller should migrate to the limited API and
choose both values deliberately.
The low-level `allocate_widths(...)` helper raises
`InfeasibleAllocationError` when a self-managed fixed command width exceeds
its core budget; 0.14 makes that refusal explicit instead of returning a
fictitious executable width.
