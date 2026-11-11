"""The /proc census bound scales with the host and is configurable (#1332 proc-scan-bound).

One fixed 30 s CPU bound for the removal census refused every removal
on a 6,723-process host under load: the scan's cost grows with the
process population, but the bound never did. The bound now resolves
from the environment, then the configuration, then a host-scaled
default (30 s floor plus 10 s per 1,000 live processes, from the
measured census costs recorded beside the validate-batch bounds).
These tests pin the resolution rule from a stubbed process count --
counted, never timed: no result here depends on how long a census
takes.
"""

from __future__ import annotations

import dataclasses
from pathlib import Path

import pytest

from wrkslots import cli


def _bare_config(bound: float | None) -> cli.Config:
    return dataclasses.replace(
        cli.Config(
            root=Path("/tmp"),
            config_path=Path("/tmp/.wrkslots.yml"),
            worktrees=Path("/tmp/worktrees"),
            control=Path("/tmp/worktrees"),
            machine="node-a",
            default_remote="origin",
            default_landed_ref="refs/remotes/origin/main",
            heartbeat_ttl_seconds=60,
            liveness_command=Path("/tmp/liveness"),
        ),
        proc_scan_bound_seconds=bound,
    )


def test_scaled_default_keeps_the_floor_and_grows_with_processes(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("WRKSLOTS_PROC_SCAN_BOUND_SECONDS", raising=False)
    monkeypatch.setattr(cli, "_host_process_count", lambda: 0)
    assert cli._proc_scan_bound_seconds() == 30.0
    monkeypatch.setattr(cli, "_host_process_count", lambda: 500)
    assert cli._proc_scan_bound_seconds() == 35.0
    # The measured incident's population: about 97 s, not a fixed 30 s.
    monkeypatch.setattr(cli, "_host_process_count", lambda: 6723)
    assert cli._proc_scan_bound_seconds() == pytest.approx(97.23)


def test_config_key_beats_the_scaled_default(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("WRKSLOTS_PROC_SCAN_BOUND_SECONDS", raising=False)
    monkeypatch.setattr(cli, "_host_process_count", lambda: 6723)
    assert cli._proc_scan_bound_seconds(_bare_config(45.0)) == 45.0
    assert cli._proc_scan_bound_seconds(_bare_config(None)) == pytest.approx(97.23)


def test_environment_beats_config_and_default(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(cli, "_host_process_count", lambda: 6723)
    monkeypatch.setenv("WRKSLOTS_PROC_SCAN_BOUND_SECONDS", "12.5")
    assert cli._proc_scan_bound_seconds(_bare_config(45.0)) == 12.5
    assert cli._proc_scan_bound_seconds() == 12.5


@pytest.mark.parametrize("raw", ["", "abc", "0", "-5", "inf", "nan"])
def test_invalid_environment_value_refuses_visibly(
    monkeypatch: pytest.MonkeyPatch, raw: str
) -> None:
    monkeypatch.setenv("WRKSLOTS_PROC_SCAN_BOUND_SECONDS", raw)
    with pytest.raises(cli.Refusal, match="WRKSLOTS_PROC_SCAN_BOUND_SECONDS"):
        cli._proc_scan_bound_seconds()


@pytest.mark.parametrize("value", [-1, 0, "30", True, float("inf"), float("nan")])
def test_bound_seconds_rejects_non_positive_or_non_numeric(value: object) -> None:
    with pytest.raises(cli.Refusal, match="proc_scan_bound_seconds"):
        cli._as_bound_seconds(value, "configuration.proc_scan_bound_seconds")


def test_bound_seconds_accepts_positive_numbers() -> None:
    assert cli._as_bound_seconds(45, "configuration.proc_scan_bound_seconds") == 45.0
    assert cli._as_bound_seconds(12.5, "configuration.proc_scan_bound_seconds") == 12.5
