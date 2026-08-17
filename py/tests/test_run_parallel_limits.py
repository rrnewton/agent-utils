"""Focused tests for independent active-step and total-CPU run limits."""

from __future__ import annotations

import argparse
import os
from pathlib import Path

import pytest

from safe_ci_dag_runner import (
    DagConfig,
    Runner,
    ResourceHint,
    RunResult,
    SpeedupLevel,
    Step,
    StepSpeedup,
    apply_plan_to_config,
    build_plan,
    cap_config_cpu_jobs,
    cap_config_max_cpus,
    dag_to_json,
    run_dag_limited,
)
from safe_ci_dag_runner import cgroup, perflog, profile_enrich
from safe_ci_dag_runner.cgroup import NoopCgroups
from safe_ci_dag_runner.cli import (
    MAX_RUN_CPUS,
    _select_max_cpus,
    _select_max_steps,
    build_parser,
    main,
)
from safe_ci_dag_runner.estimates import Planner


def _run_args(**overrides: object) -> argparse.Namespace:
    values: dict[str, object] = {
        "max_cpus": None,
        "max_steps": None,
        "max_mem": None,
        "cores": None,
    }
    values.update(overrides)
    return argparse.Namespace(**values)


def _sleep_cfg(*, count: int = 4, width: int = 1) -> DagConfig:
    return DagConfig(
        steps=tuple(
            Step(
                "g",
                f"s{index}",
                "",
                "sleep 0.2",
                hint=ResourceHint(preferred_inner_jobs=width),
                jobs_flag="",
            )
            for index in range(count)
        )
    )


def test_run_parser_separates_bare_max_steps_and_max_cpus() -> None:
    parsed = build_parser().parse_args(["run", "--dag", "dag.json", "-s2", "-j8"])
    assert parsed.max_steps == 2
    assert parsed.max_cpus == 8


@pytest.mark.parametrize(
    "args",
    [
        ["run", "--dag", "dag.json", "--max-steps", "0"],
        ["run", "--dag", "dag.json", "--max-cpus", "0"],
        ["run", "--dag", "dag.json", "--max-cpus", str(MAX_RUN_CPUS + 1)],
        ["run", "--dag", "dag.json", "--jobs", "0"],
    ],
)
def test_run_parser_rejects_invalid_limits(args: list[str]) -> None:
    with pytest.raises(SystemExit) as exc:
        build_parser().parse_args(args)
    assert exc.value.code == 2


def test_run_help_names_both_independent_limits(capsys: pytest.CaptureFixture[str]) -> None:
    with pytest.raises(SystemExit) as exc:
        build_parser().parse_args(["run", "--help"])
    assert exc.value.code == 0
    help_text = capsys.readouterr().out
    assert "maximum active DAG steps" in help_text
    assert "maximum total CPU cores" in help_text
    assert "shared 90% slice" in " ".join(help_text.split())
    assert "--max-cpus" in help_text
    assert "--jobs" not in help_text


def test_hidden_run_jobs_alias_and_conflict() -> None:
    legacy = build_parser().parse_args(
        ["run", "--dag", "dag.json", "--jobs", "7"]
    )
    assert legacy.max_cpus == 7
    equal = build_parser().parse_args(
        ["run", "--dag", "dag.json", "--max-cpus", "7", "--jobs", "7"]
    )
    assert equal.max_cpus == 7
    with pytest.raises(SystemExit) as exc:
        build_parser().parse_args(
            ["run", "--dag", "dag.json", "--max-cpus", "7", "--jobs", "8"]
        )
    assert exc.value.code == 2


def test_default_max_cpus_takes_tightest_container_affinity_and_shared_slice(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr("safe_ci_dag_runner.cli.container_core_budget", lambda: 8)
    monkeypatch.setattr(cgroup, "aggregate_slice_max_cpus", lambda: 6)
    assert _select_max_cpus(_run_args()) == 6
    assert _select_max_cpus(_run_args(max_cpus=10)) == 10
    assert _select_max_cpus(_run_args(max_cpus=10, cores=3)) == 3


def test_python_api_keeps_cpu_jobs_compatibility_aliases() -> None:
    cfg = DagConfig(steps=(Step("g", "one", "", "true"),))
    canonical = Runner(
        cfg, max_steps=1, max_cpus=2, cgroups=NoopCgroups(), verbosity=0
    )
    legacy = Runner(
        cfg, max_steps=1, cpu_jobs=2, cgroups=NoopCgroups(), verbosity=0
    )
    assert canonical.max_cpus == legacy.max_cpus == 2
    assert canonical.cpu_jobs == legacy.cpu_jobs == 2
    assert (
        Runner(
            cfg,
            max_steps=1,
            max_cpus=2,
            cpu_jobs=2,
            cgroups=NoopCgroups(),
            verbosity=0,
        ).max_cpus
        == 2
    )
    assert cap_config_cpu_jobs(cfg, 2) == cap_config_max_cpus(cfg, 2)
    assert run_dag_limited(cfg, max_steps=1, cpu_jobs=2, verbosity=0).ok
    assert run_dag_limited(
        cfg, max_steps=1, max_cpus=2, cpu_jobs=2, verbosity=0
    ).ok
    with pytest.raises(TypeError, match="disagree"):
        run_dag_limited(
            cfg, max_steps=1, max_cpus=2, cpu_jobs=3, verbosity=0
        )
    with pytest.raises(TypeError, match="disagree"):
        Runner(
            cfg,
            max_steps=1,
            max_cpus=2,
            cpu_jobs=3,
            cgroups=NoopCgroups(),
            verbosity=0,
        )


def test_max_mem_and_explicit_max_steps_use_tighter_ceiling(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    cfg = _sleep_cfg(count=1)
    monkeypatch.setattr("safe_ci_dag_runner.cli.jobs_for_budget", lambda _cfg, _budget: (5, 123))
    assert _select_max_steps(cfg, _run_args(), 7) == 7
    assert _select_max_steps(cfg, _run_args(max_mem="1G"), 7) == 5
    assert _select_max_steps(cfg, _run_args(max_steps=3, max_mem="1G"), 7) == 3
    assert _select_max_steps(cfg, _run_args(max_steps=9, max_mem="1G"), 7) == 5


def test_max_steps_and_max_cpus_constrain_independently() -> None:
    step_limited = run_dag_limited(
        _sleep_cfg(), max_steps=2, max_cpus=4, verbosity=0
    )
    assert step_limited.ok
    assert step_limited.max_concurrent_steps == 2

    cpu_limited = run_dag_limited(
        _sleep_cfg(), max_steps=4, max_cpus=2, verbosity=0
    )
    assert cpu_limited.ok
    assert cpu_limited.max_concurrent_steps == 2

    width_limited = run_dag_limited(
        _sleep_cfg(width=2), max_steps=4, max_cpus=4, verbosity=0
    )
    assert width_limited.ok
    assert width_limited.max_concurrent_steps == 2


def test_default_step_cpu_count_is_charged_as_effective_width() -> None:
    cfg = DagConfig(
        steps=(
            Step("g", "one", "", 'check() { [ "$#" -eq 0 ]; }; check; sleep 0.2'),
            Step("g", "two", "", 'check() { [ "$#" -eq 0 ]; }; check; sleep 0.2'),
        ),
        default_step_cpu_count=4,
    )
    result = run_dag_limited(cfg, max_steps=2, max_cpus=4, verbosity=0)
    assert result.ok
    assert result.max_concurrent_steps == 1


def test_default_step_profiles_the_effective_cpu_width() -> None:
    cfg = DagConfig(
        steps=(Step("g", "default-width", "", "true"),),
        default_step_cpu_count=1,
    )
    result = run_dag_limited(
        cfg,
        max_steps=1,
        max_cpus=8,
        cgroups=_BoxedRecordingCgroups(),
        verbosity=0,
    )
    assert result.ok
    assert len(result.step_profile_rows) == 1
    assert result.step_profile_rows[0]["inner_jobs"] == 1
    assert result.step_profile_rows[0]["quota_utilization_pct"] == 0.0


class _RecordingCgroups(NoopCgroups):
    def __init__(self) -> None:
        self.cpu_counts: list[int | None] = []

    def prepare_command(
        self,
        tag: str,
        cmd: str,
        mem_max: int | None = None,
        cpu_count: int | None = None,
    ) -> str:
        self.cpu_counts.append(cpu_count)
        return cmd


class _BoxedRecordingCgroups(_RecordingCgroups):
    enabled = True

    def cpu_stats(self, tag: str) -> dict[str, int]:
        return {
            "usage_usec": 0,
            "user_usec": 0,
            "system_usec": 0,
            "throttled_usec": 0,
        }


def test_oversized_width_jobs_flag_and_default_cpu_cap_are_clamped(
    capsys: pytest.CaptureFixture[str],
) -> None:
    cfg = DagConfig(
        steps=(
            Step(
                "g",
                "wide",
                "",
                'check() { [ "$*" = "-j2" ]; }; check',
                hint=ResourceHint(preferred_inner_jobs=8),
                jobs_flag="-j%d",
            ),
        ),
        default_step_cpu_count=8,
    )
    capped = cap_config_max_cpus(cfg, 2)
    assert capped.steps[0].hint.preferred_inner_jobs == 2
    assert capped.default_step_cpu_count == 2

    manager = _RecordingCgroups()
    result = run_dag_limited(
        cfg, max_steps=1, max_cpus=2, cgroups=manager, verbosity=0
    )
    assert result.ok
    assert manager.cpu_counts == [2]
    warnings = capsys.readouterr().err
    assert "preferred_inner_jobs=8" in warnings
    assert "default_step_cpu_count=8" in warnings


def test_plan_application_preserves_top_level_cpu_policy_before_clamp() -> None:
    cfg = DagConfig(steps=(Step("g", "one", "", "true"),), default_step_cpu_count=8)
    applied = apply_plan_to_config(cfg, build_plan(cfg, {}))
    assert applied.default_step_cpu_count == 8
    assert cap_config_max_cpus(applied, 2).default_step_cpu_count == 2


def test_small_default_cap_is_applied_after_run_cpu_job_clamp(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    path = tmp_path / "dag.json"
    path.write_text(
        dag_to_json(
            DagConfig(
                steps=(Step("g", "one", "", "true"),),
                default_step_cpu_count=8,
            )
        )
    )
    observed: list[int | None] = []

    def fake_run(cfg: DagConfig, **_kwargs: object) -> RunResult:
        observed.append(cfg.default_step_cpu_count)
        return RunResult(ok=True, wall_s=0.0)

    monkeypatch.setattr("safe_ci_dag_runner.cli.run_dag_limited", fake_run)
    rc = main(
        [
            "run",
            "--dag",
            str(path),
            "--max-cpus",
            "2",
            "--max-steps",
            "1",
            "--small-default-cap",
            "--unsafe-no-cgroups",
            "--no-profile",
            "--no-profile-feedback",
            "--quiet",
        ]
    )
    assert rc == 0
    assert observed == [1]


@pytest.mark.parametrize(
    ("quota", "affinity", "expected"),
    [
        ("50000_100000", 8, 1),
        ("150000_100000", 8, 1),
        ("250000_100000", 8, 2),
        ("800000_100000", 3, 3),
    ],
)
def test_container_core_budget_floors_fractional_quota_and_mins_affinity(
    monkeypatch: pytest.MonkeyPatch, quota: str, affinity: int, expected: int
) -> None:
    monkeypatch.setattr(perflog, "effective_cpu_quota", lambda: quota)
    monkeypatch.setattr(perflog, "nproc", lambda: affinity)
    assert profile_enrich.container_core_budget() == expected


@pytest.mark.parametrize(("percent", "expected"), [(50, 1), (150, 1), (250, 2)])
def test_shared_slice_max_cpus_floors_fractional_quota(
    monkeypatch: pytest.MonkeyPatch, percent: int, expected: int
) -> None:
    monkeypatch.setattr(cgroup, "cpu_quota_percent", lambda: percent)
    assert cgroup.aggregate_slice_max_cpus() == expected
    assert cgroup.aggregate_slice_cpu_jobs() == expected


def test_cpa_omits_curve_entirely_above_strict_cpu_budget() -> None:
    speedup = StepSpeedup(
        step="g.wide",
        baseline_inner_jobs=4,
        recommended_inner_jobs=8,
        measured_effective_cores=8.0,
        regression_inner_jobs=None,
        levels=(
            SpeedupLevel(
                inner_jobs=4,
                samples=1,
                wall_s=8.0,
                raw_wall_s=8.0,
                wall_min_s=8.0,
                wall_max_s=8.0,
                cpu_s=32.0,
                effective_cores=4.0,
                throttled_s=0.0,
                speedup=1.0,
            ),
            SpeedupLevel(
                inner_jobs=8,
                samples=1,
                wall_s=4.0,
                raw_wall_s=4.0,
                wall_min_s=4.0,
                wall_max_s=4.0,
                cpu_s=32.0,
                effective_cores=8.0,
                throttled_s=0.0,
                speedup=2.0,
            ),
        ),
    )
    cfg = DagConfig(
        steps=(
            Step(
                "g",
                "wide",
                "",
                "true",
                hint=ResourceHint(est_duration_s=12.0, preferred_inner_jobs=8),
            ),
        )
    )
    plan = build_plan(
        cfg,
        {},
        planner=Planner.CPA,
        speedups={"g.wide": speedup},
        core_budget=2,
    )
    assert plan.entries[0].alloc_inner_jobs == 2
    assert plan.entries[0].speedup is None


def test_cpa_charges_default_step_cpu_count_for_curveless_step() -> None:
    cfg = DagConfig(
        steps=(Step("g", "default", "", "true", hint=ResourceHint(est_duration_s=1.0)),),
        default_step_cpu_count=4,
    )
    plan = build_plan(cfg, {}, planner=Planner.CPA, core_budget=4)
    assert plan.entries[0].alloc_inner_jobs == 4


def test_scope_reexec_requests_and_carries_exact_max_cpus(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    captured: list[str] = []

    def fake_execvp(_program: str, argv: list[str]) -> None:
        captured.extend(argv)
        raise OSError("planted exec stop")

    monkeypatch.setattr(cgroup, "systemd_scope_available", lambda: True)
    monkeypatch.setattr(cgroup, "ensure_aggregate_slice", lambda naming: False)
    monkeypatch.setattr(os, "execvp", fake_execvp)
    assert not cgroup.reexec_in_scope(
        ["runner", "run"],
        memory_max=1024,
        cpu_count=3,
        skip_in_ci=False,
    )
    assert "CPUQuota=300%" in captured
    assert f"--setenv={cgroup.EXPECTED_OUTER_CPU_COUNT_ENV}=3" in captured


def test_scope_limit_audit_requires_exact_cpu_max(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    (tmp_path / "memory.max").write_text("104857600")
    (tmp_path / "memory.swap.max").write_text("0")
    (tmp_path / "memory.oom.group").write_text("1")
    (tmp_path / "cpu.max").write_text("200000 100000")
    monkeypatch.setattr(cgroup, "scope_cgroup_from_self", lambda naming: tmp_path)
    assert cgroup.verify_scope_limits(104857600, 2)
    (tmp_path / "cpu.max").write_text("300000 100000")
    assert not cgroup.verify_scope_limits(104857600, 2)
