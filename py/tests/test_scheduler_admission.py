"""Integration tests: memory-primary admission wired into ``run_dag``.

The whole box is admitted BEFORE any step launches. With no per-step ``rss_baseline_bytes``
the box footprint is the configured floor (8 GiB), so these tests drive GRANT / QUEUE / REFUSE
purely through the injected ``Admitter`` (synthetic ``/proc/meminfo`` + an explicit ledger
budget), and assert the observable outcome: a refused/queued box runs NO step, a granted box
runs and leaves the ledger empty (the reservation is released).
"""

from __future__ import annotations

import os
from pathlib import Path

from safe_ci_dag_runner.admission import Admitter
from safe_ci_dag_runner.model import DagConfig, ResourceHint, Step
from safe_ci_dag_runner.scheduler import run_dag

_GIB = 1024**3


def _meminfo(total_gib: float, avail_gib: float) -> str:
    return (
        f"MemTotal:       {int(total_gib * _GIB / 1024)} kB\n"
        f"MemAvailable:   {int(avail_gib * _GIB / 1024)} kB\n"
    )


def _sentinel_cfg(marker: Path) -> DagConfig:
    return DagConfig(
        steps=(
            Step("g", "touch", "", f"touch {marker}", hint=ResourceHint()),
        )
    )


def _admitter(tmp_path: Path, *, budget_gib: float, avail_gib: float, pid: int) -> Admitter:
    return Admitter(
        state_dir=tmp_path,
        meminfo_reader=lambda: _meminfo(1000, avail_gib),
        mem_budget_bytes=int(budget_gib * _GIB),
        pid=pid,
    )


def test_admitted_box_runs_and_releases_reservation(tmp_path: Path) -> None:
    marker = tmp_path / "ran"
    adm = _admitter(tmp_path, budget_gib=100, avail_gib=100, pid=os.getpid())
    res = run_dag(
        _sentinel_cfg(marker), jobs=1, verbosity=0, admitter=adm, admission_box="unit"
    )
    assert res.ok
    assert marker.exists()  # the box actually ran its step
    # The reservation is released after the run: a fresh view of the shared ledger is empty.
    assert adm.snapshot()["active_boxes"] == 0


def test_refused_box_runs_no_step(tmp_path: Path) -> None:
    marker = tmp_path / "ran"
    # Budget (1 GiB) is smaller than the 8 GiB footprint floor -> the box can NEVER fit -> REFUSE.
    adm = _admitter(tmp_path, budget_gib=1, avail_gib=100, pid=os.getpid())
    res = run_dag(
        _sentinel_cfg(marker), jobs=1, verbosity=0, admitter=adm, admission_box="giant"
    )
    assert not res.ok  # an unrun box is a failure, never a silent pass
    assert res.outcomes == ()  # no step produced an outcome
    assert not marker.exists()  # nothing launched


def test_queued_box_gives_up_without_running(tmp_path: Path) -> None:
    marker = tmp_path / "ran"
    # A live holder occupies most of a 10 GiB budget; the 8 GiB box would overcommit -> QUEUE.
    holder = _admitter(tmp_path, budget_gib=10, avail_gib=100, pid=os.getpid())
    assert holder.request(6 * _GIB, box="holder").verdict.value == "grant"
    box_adm = _admitter(tmp_path, budget_gib=10, avail_gib=100, pid=os.getpid())
    # wait_s=0 -> give up after the first QUEUE verdict (no real sleeping).
    res = run_dag(
        _sentinel_cfg(marker),
        jobs=1,
        verbosity=0,
        admitter=box_adm,
        admission_box="waiter",
        admission_wait_s=0.0,
    )
    assert not res.ok
    assert res.outcomes == ()
    assert not marker.exists()  # queued -> never contended -> never ran


def test_no_admitter_is_unchanged_behaviour(tmp_path: Path) -> None:
    marker = tmp_path / "ran"
    res = run_dag(_sentinel_cfg(marker), jobs=1, verbosity=0)  # admitter=None
    assert res.ok
    assert marker.exists()
