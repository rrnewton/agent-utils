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
