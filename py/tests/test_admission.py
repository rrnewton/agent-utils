"""Tests for memory-primary box admission control.

Live memory is fed synthetically via an injectable ``/proc/meminfo`` reader, and the
reservation ledger runs against a shared ``state_dir`` in ``tmp_path`` so the
overcommit, live-memory, and PID-reclamation branches are checked directly.
"""

from __future__ import annotations

import os
import subprocess
from collections.abc import Callable, Iterator
from pathlib import Path

import pytest

from safe_ci_dag_runner.admission import (
    Admitter,
    Verdict,
    main,
    read_meminfo,
)

_GIB = 1024**3


def _meminfo(total_gib: float, avail_gib: float) -> str:
    return (
        f"MemTotal:       {int(total_gib * _GIB / 1024)} kB\n"
        f"MemFree:        {int(avail_gib * _GIB / 1024)} kB\n"
        f"MemAvailable:   {int(avail_gib * _GIB / 1024)} kB\n"
    )


def _reader(total_gib: float, avail_gib: float) -> Callable[[], str]:
    return lambda: _meminfo(total_gib, avail_gib)


@pytest.fixture
def live_pid() -> "Iterator[int]":
    p = subprocess.Popen(["sleep", "30"])
    try:
        yield p.pid
    finally:
        p.terminate()
        p.wait()


def test_read_meminfo_parses_bytes_and_tolerates_missing() -> None:
    mem = read_meminfo(_reader(100, 40))
    assert mem.total == 100 * _GIB
    assert mem.available == 40 * _GIB
    # Missing MemAvailable -> 0 (safe direction: queue rather than silently admit).
    partial = read_meminfo(lambda: "MemTotal:  1024 kB\n")
    assert partial.total == 1024 * 1024
    assert partial.available == 0


def test_grant_when_it_fits_and_records_reservation(tmp_path: Path) -> None:
    adm = Admitter(state_dir=tmp_path, meminfo_reader=_reader(100, 90), pid=os.getpid())
    d = adm.request(10 * _GIB, box="validate")
    assert d.verdict is Verdict.GRANT
    assert d.reservation_id is not None
    assert "GRANTED" in d.message
    # The reservation is now visible in live state.
    snap = adm.snapshot()
    assert snap["active_boxes"] == 1
    assert snap["reserved_bytes"] == 10 * _GIB


def test_refuse_when_request_exceeds_whole_host_budget(tmp_path: Path) -> None:
    # 100 GiB total * 0.85 = 85 GiB budget; a 90 GiB box can NEVER fit.
    adm = Admitter(state_dir=tmp_path, meminfo_reader=_reader(100, 100), pid=os.getpid())
    d = adm.request(90 * _GIB, box="giant")
    assert d.verdict is Verdict.REFUSE
    assert d.bound_by == "over-budget"
    assert "REFUSED" in d.message and "Ask for less" in d.message


def test_queue_when_reservation_ledger_would_overcommit(tmp_path: Path, live_pid: int) -> None:
    # Small explicit budget so a second box overcommits the LEDGER (not live memory).
    # First reservation owned by a live background pid so it is not reclaimed.
    holder = Admitter(state_dir=tmp_path, meminfo_reader=_reader(100, 100),
                      mem_budget_bytes=30 * _GIB, pid=live_pid)
    assert holder.request(20 * _GIB, box="first").verdict is Verdict.GRANT
    me = Admitter(state_dir=tmp_path, meminfo_reader=_reader(100, 100),
                  mem_budget_bytes=30 * _GIB, pid=os.getpid())
    d = me.request(20 * _GIB, box="second")  # 20 + 20 > 30 budget
    assert d.verdict is Verdict.QUEUE
    assert d.bound_by == "reservation-budget"
    assert d.position == 2
    assert "QUEUED (position 2)" in d.message


def test_queue_when_live_host_too_full(tmp_path: Path) -> None:
    # Budget is ample (85 GiB) but only 5 GiB is free live -> the live gate binds.
    adm = Admitter(state_dir=tmp_path, meminfo_reader=_reader(100, 5), pid=os.getpid())
    d = adm.request(10 * _GIB, box="validate")
    assert d.verdict is Verdict.QUEUE
    assert d.bound_by == "live-memory"
    assert "only" in d.message


def test_release_frees_the_reservation(tmp_path: Path) -> None:
    adm = Admitter(state_dir=tmp_path, meminfo_reader=_reader(100, 90),
                   mem_budget_bytes=15 * _GIB, pid=os.getpid())
    d1 = adm.request(10 * _GIB, box="a")
    assert d1.verdict is Verdict.GRANT
    # Second 10 GiB overcommits the 15 GiB budget -> queue.
    assert adm.request(10 * _GIB, box="b").verdict is Verdict.QUEUE
    assert d1.reservation_id is not None
    adm.release(d1.reservation_id)
    assert adm.request(10 * _GIB, box="b").verdict is Verdict.GRANT  # room again


def test_dead_owner_reservation_is_reclaimed(tmp_path: Path) -> None:
    dead = subprocess.Popen(["true"])
    dead.wait()  # reaped -> pid now dead
    ghost = Admitter(state_dir=tmp_path, meminfo_reader=_reader(100, 100),
                     mem_budget_bytes=15 * _GIB, pid=dead.pid)
    assert ghost.request(12 * _GIB, box="ghost").verdict is Verdict.GRANT
    live = Admitter(state_dir=tmp_path, meminfo_reader=_reader(100, 100),
                    mem_budget_bytes=15 * _GIB, pid=os.getpid())
    # Without reclamation 12 + 12 > 15 would queue; the dead owner's reservation is dropped.
    assert live.request(12 * _GIB, box="live").verdict is Verdict.GRANT


def test_cli_exit_codes(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], monkeypatch: pytest.MonkeyPatch
) -> None:
    # Point the CLI's default state dir + live meminfo at the fixture via env/monkeypatch.
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    monkeypatch.setattr(
        "safe_ci_dag_runner.admission._read_meminfo_live", lambda: _meminfo(100, 90)
    )
    assert main(["request", "--mem-gib", "10", "--box", "validate"]) == 0  # GRANT
    assert main(["request", "--mem-gib", "999"]) == 1  # REFUSE (over budget)
    monkeypatch.setattr(
        "safe_ci_dag_runner.admission._read_meminfo_live", lambda: _meminfo(100, 2)
    )
    assert main(["request", "--mem-gib", "10"]) == 75  # QUEUE (live-memory)
    assert main(["status"]) == 0
