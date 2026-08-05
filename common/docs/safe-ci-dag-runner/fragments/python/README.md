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
