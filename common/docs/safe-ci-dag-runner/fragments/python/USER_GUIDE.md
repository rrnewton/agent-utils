## Installation and library use

```sh
python3 -m pip install safe-ci-dag-runner
```

Python 3.10 or newer is required. Installation provides both console commands,
the typed `safe_ci_dag_runner` package, and this guide as package data.

```python
from safe_ci_dag_runner import DagConfig, Step, run_dag, to_ascii

dag = DagConfig(steps=(Step("build", "app", "compile", "make build"),))
print(to_ascii(dag))
result = run_dag(dag, jobs=1)
assert result.ok
```

The package also exports strict JSON/YAML conversion, resource sizing,
pluggable containment and metrics protocols, plan construction, and profile
analysis. Applications that need enforced containment should run on Linux with
cgroup v2 and a delegated systemd user scope.
