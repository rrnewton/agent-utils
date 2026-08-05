# agent-utils for Python

This directory contains the independently installable Python editions of the
agent-utils command-line tools. Each distribution owns one import package, its
own version and documentation, and only the console commands that belong to it.

| Distribution | Import package | Commands | Purpose |
| --- | --- | --- | --- |
| `safe-ci-dag-runner` | `safe_ci_dag_runner` | `safe-ci-dag-runner`, `cpuset-alloc` | Run and inspect resource-aware CI DAGs; the companion allocator reserves isolated CPU sets for benchmarks. |
| `tick-hub` | `tick_hub` | `tick-hub` | Evaluate cadenced reminders and health checks in one deterministic tick. |
| `pr-landing-planner` | `pr_landing_planner` | `pr-landing-planner` | Produce an advisory, conflict-aware pull-request landing plan. |
| `agent-team-timeline` | `agent_team_timeline` | `agent-team-timeline` | Build a durable, zoomable local timeline from coordinator and subagent transcripts. |

Install a tool from its project directory during development:

```sh
python3 -m pip install ./py/safe_ci_dag_runner
python3 -m pip install ./py/tick_hub
python3 -m pip install ./py/pr_landing_planner
python3 -m pip install ./py/agent_team_timeline
```

The root `pyproject.toml` contains shared type-checker configuration only; it is
deliberately not a publishable aggregate distribution. Repository development
continues to use the flat source tree:

```sh
cd py
python3 -m mypy .
python3 -m pytest -q
```

Run `make check-python-packages` from the repository root to build each wheel and
source distribution in isolation, rebuild the wheel from the source archive,
inspect its contents, install it without dependency or network access, and
smoke all of its declared commands. The invoking interpreter must
already provide `setuptools>=77`; the offline check never downloads its build
backend.
