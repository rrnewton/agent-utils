"""Independent per-platform wall-time scaling.

Wall time measures elapsed scheduling and I/O delay; CPU time measures occupied cores.  The two
machine effects therefore need independent knobs, with one canonical graph and scaling only at
execution time.  Whole-second scaling rounds half up, matching CPU-budget enforcement.
"""

from __future__ import annotations

import pytest

from dagrun.cli import build_parser
from dagrun.model import (
    DEFAULT_WALL_TIMEOUT_MULTIPLIER,
    WALL_TIMEOUT_MULTIPLIER_ENV,
    WALL_TIMEOUT_PLATFORM_ENV,
    DagConfig,
    Step,
    resolve_wall_timeout_multiplier,
    resolved_wall_timeout,
    scale_wall_timeout,
)


def step(*, timeout: int = 0, cpu_timeout: int = 0) -> Step:
    return Step("g", "j", "d", "true", timeout=timeout, cpu_timeout=cpu_timeout)


def test_unity_is_a_strict_no_op() -> None:
    cfg = DagConfig(steps=())
    assert DEFAULT_WALL_TIMEOUT_MULTIPLIER == 1.0
    assert cfg.wall_timeout_multiplier == 1.0
    assert cfg.wall_timeout_platform == ""
    assert resolved_wall_timeout(step(timeout=57), 0, 2.0, 1.0) == 57


def test_cpu_and_wall_multipliers_are_independent_for_an_explicit_wall_bound() -> None:
    declared = step(timeout=57, cpu_timeout=22)
    assert resolved_wall_timeout(declared, 0, 3.0, 1.0) == 57
    assert resolved_wall_timeout(declared, 0, 1.0, 2.0) == 114


def test_a_cpu_derived_wall_bound_composes_both_machine_effects() -> None:
    # The CPU multiplier first preserves the existing 3x CPU backstop relationship; the
    # independent wall multiplier then accounts for elapsed-time slowdown.
    assert resolved_wall_timeout(step(cpu_timeout=700), 0, 2.0, 1.5) == 6300


def test_rounding_is_whole_second_half_up_and_cannot_disable_a_live_bound() -> None:
    assert scale_wall_timeout(57, 1.5) == 86
    assert scale_wall_timeout(7, 1.5) == 11
    assert scale_wall_timeout(1, 0.01) == 1
    assert scale_wall_timeout(0, 2.0) == 0


def test_environment_and_explicit_resolution_are_typed() -> None:
    env = {
        WALL_TIMEOUT_MULTIPLIER_ENV: "1.5",
        WALL_TIMEOUT_PLATFORM_ENV: "loaded-host",
    }
    assert resolve_wall_timeout_multiplier(None, env) == (1.5, "loaded-host")
    assert resolve_wall_timeout_multiplier(2.0, env) == (2.0, "loaded-host")
    assert resolve_wall_timeout_multiplier(None, {}) == (1.0, "")


def test_run_cli_accepts_the_independent_wall_multiplier() -> None:
    parsed = build_parser().parse_args(
        ["run", "--dag", "pipeline.yaml", "--wall-timeout-multiplier", "1.75"]
    )
    assert parsed.wall_timeout_multiplier == 1.75


@pytest.mark.parametrize("raw", ["nope", "0", "-1", "nan", "inf"])
def test_invalid_environment_values_are_refused(raw: str) -> None:
    with pytest.raises(ValueError):
        resolve_wall_timeout_multiplier(None, {WALL_TIMEOUT_MULTIPLIER_ENV: raw})


@pytest.mark.parametrize("value", [0.0, -1.0, float("nan"), float("inf")])
def test_invalid_explicit_values_are_refused(value: float) -> None:
    with pytest.raises(ValueError):
        resolve_wall_timeout_multiplier(value, {})
