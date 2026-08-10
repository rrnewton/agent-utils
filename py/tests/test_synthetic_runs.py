"""A battery of synthetic-DAG runs (small, fast, deterministic) exercising the runner + CLI.

Steps are `sleep`-based (time, not CPU burn) with controllable per-step duration and
env-driven intra-step parallelism, so the whole file runs in a few seconds.
"""

from __future__ import annotations

import contextlib
import io
import tempfile
from pathlib import Path

from safe_ci_dag_runner import DagConfig, ResourceHint, Step, dag_to_json, run_dag
from safe_ci_dag_runner.cli import main


# --------------------------------------------------------------- synthetic builders
def busy_cmd(seconds: float) -> str:
    """POSIX sh: spawn $INNER parallel sleepers (default 1), each ~seconds, then wait."""
    return (
        'n="${INNER:-1}"; i=0; '
        f'while [ "$i" -lt "$n" ]; do sleep {seconds:g} & i=$((i+1)); done; wait'
    )


def sleeper(
    group: str,
    job: str,
    seconds: float,
    *,
    deps: list[str] | None = None,
    resource: str | None = None,
    inner: int = 1,
) -> Step:
    resources: dict[str, int] = {resource: 1} if resource else {}
    hint = ResourceHint(
        est_duration_s=seconds * max(1, inner),
        resources=resources,
        preferred_inner_jobs=(inner if inner > 1 else None),
    )
    cmd = busy_cmd(seconds) if inner > 1 else f"sleep {seconds:g}"
    env: dict[str, str] = {"INNER": str(inner)} if inner > 1 else {}
    # busy_cmd drives its inner parallelism via the INNER env var, so the concurrency flag must
    # NOT be appended to the command (an empty jobs_flag template disables the append).
    jobs_flag = "" if inner > 1 else None
    return Step(
        group, job, f"{group}.{job}", cmd, deps=list(deps or []), env=env, hint=hint, jobs_flag=jobs_flag
    )


def diamond(seconds: float = 0.1) -> DagConfig:
    return DagConfig(
        steps=(
            sleeper("g", "a", seconds),
            sleeper("g", "b", seconds, deps=["g.a"]),
            sleeper("g", "c", seconds, deps=["g.a"]),
            sleeper("g", "d", seconds, deps=["g.b", "g.c"]),
        )
    )


def wide(n: int, seconds: float = 0.1, *, cap: int | None = None) -> DagConfig:
    caps: dict[str, int] = {} if cap is None else {"slot": cap}
    res = None if cap is None else "slot"
    steps: list[Step] = [sleeper("g", "root", seconds)]
    for i in range(n):
        steps.append(sleeper("leaf", f"j{i}", seconds, deps=["g.root"], resource=res))
    return DagConfig(steps=tuple(steps), resource_caps=caps)


def chain(n: int, seconds: float = 0.05) -> DagConfig:
    steps: list[Step] = []
    for i in range(n):
        deps = [f"c.s{i - 1}"] if i else []
        steps.append(sleeper("c", f"s{i}", seconds, deps=deps))
    return DagConfig(steps=tuple(steps))


def _run_cli(args: list[str]) -> int:
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        return main(args)


# --------------------------------------------------------------- tests
def test_diamond_all_pass() -> None:
    r = run_dag(diamond(0.05), jobs=4, verbosity=0)
    assert r.ok
    assert len(r.outcomes) == 4 and all(o.ok for o in r.outcomes)


def test_wide_all_pass() -> None:
    r = run_dag(wide(8, 0.05), jobs=8, verbosity=0)
    assert r.ok and len(r.outcomes) == 9


def test_wide_cap_serializes() -> None:
    # 4 leaves demanding a cap-1 resource must run one at a time.
    r = run_dag(wide(4, 0.15, cap=1), jobs=8, verbosity=0)
    assert r.ok
    assert r.wall_s >= 0.4  # ~4 * 0.15 serialized (loose lower bound)


def test_chain_is_serial() -> None:
    r = run_dag(chain(5, 0.05), jobs=8, verbosity=0)
    assert r.ok and r.wall_s >= 0.2  # 5 * 0.05 forced serial by deps (loose)


def test_intra_step_parallelism_runs_concurrently() -> None:
    # Compare like-for-like runner and teardown overhead instead of imposing an absolute wall-time
    # ceiling: ownership sweeps and host load are independent of the inner sleepers themselves.
    parallel = run_dag(
        DagConfig(steps=(sleeper("g", "fan", 0.1, inner=4),)), jobs=1, verbosity=0
    )
    serial = run_dag(
        DagConfig(
            steps=(
                Step(
                    "g",
                    "serial",
                    "g.serial",
                    "sleep 0.1; sleep 0.1; sleep 0.1; sleep 0.1",
                ),
            )
        ),
        jobs=1,
        verbosity=0,
    )
    assert parallel.ok and serial.ok
    assert parallel.wall_s + 0.15 < serial.wall_s


def test_dep_failure_skips_dependents() -> None:
    cfg = DagConfig(steps=(Step("g", "a", "", "false"), Step("g", "b", "", "true", deps=["g.a"])))
    r = run_dag(cfg, jobs=2, verbosity=0)
    assert not r.ok and "g.b" in r.skipped


def test_eager_exit_aborts_inflight() -> None:
    cfg = DagConfig(steps=(Step("g", "fast", "", "sleep 0.2; false"), Step("g", "slow", "", "sleep 5")))
    r = run_dag(cfg, jobs=2, verbosity=0)
    outs = {o.tag: o for o in r.outcomes}
    assert not r.ok and outs["g.slow"].aborted is True


def test_cli_run_ok_from_file() -> None:
    with tempfile.TemporaryDirectory() as d:
        path = Path(d) / "dag.json"
        path.write_text(dag_to_json(diamond(0.05)), encoding="utf-8")
        # --allow-cgroup-failure so the default-on boxing does NOT re-exec into a systemd scope
        # (which would replace the in-process pytest runner).
        assert _run_cli(["run", "--dag", str(path), "-q", "--allow-cgroup-failure"]) == 0


def test_cli_run_reports_failure_exit_1() -> None:
    with tempfile.TemporaryDirectory() as d:
        path = Path(d) / "dag.json"
        path.write_text('{"steps": [{"group": "g", "job": "x", "cmd": "false"}]}', encoding="utf-8")
        assert _run_cli(["run", "--dag", str(path), "-q", "--allow-cgroup-failure"]) == 1


def test_cli_render_commands_exit_0() -> None:
    with tempfile.TemporaryDirectory() as d:
        path = Path(d) / "dag.json"
        path.write_text(dag_to_json(wide(3, 0.01)), encoding="utf-8")
        for cmd in ("list", "ascii", "dot", "json"):
            assert _run_cli([cmd, "--dag", str(path)]) == 0
    assert _run_cli(["quickstart"]) == 0


def test_stress_wide_completes() -> None:
    r = run_dag(wide(20, 0.05), jobs=8, verbosity=0)
    assert r.ok and len(r.outcomes) == 21
