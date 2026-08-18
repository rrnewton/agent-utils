## Install

```sh
python3 -m pip install safe-ci-dag-runner
```

Python 3.10 or newer is required. The installation provides the
`safe-ci-dag-runner` and `cpuset-alloc` console commands, an importable
`safe_ci_dag_runner` package, and typing metadata.

## Python API

The model, serializer, planner, scheduler, and visualization helpers are public:

```python
from safe_ci_dag_runner import DagConfig, Step, to_ascii

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
