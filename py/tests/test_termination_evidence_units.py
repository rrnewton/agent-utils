"""Termination evidence must name its unit and compare like with like.

WHY THIS EXISTS. A step can cross two different bounds: a wall ceiling and a
CPU budget. They are enforced on different quantities -- ``step.timeout``
against elapsed wall seconds, ``cpu_timeout`` against cgroup ``cpu.stat``
CPU-seconds -- and for the same step those two numbers differ, because a step
that keeps roughly one core busy accrues wall faster than CPU.

The evidence recorded at the kill used to carry a single ``elapsed_s`` that was
always WALL, next to a ``limit_s`` that was CPU for a CPU budget. Nothing in
the record said which was which, so the natural reading -- compare the two
printed numbers -- silently compared a wall figure against a CPU bound. On one
measured trip that produced a recorded 354.587 against a 300 CPU-second limit
while the step had actually consumed about 308 CPU-seconds, and the same
record also exceeded the whole run's CPU rollup, which is arithmetically
impossible for a single node and is what exposed the defect.

So the assertions below are about keeping the record self-describing:
  * a CPU breach reports the CPU-seconds it actually compared, not wall,
  * every recorded quantity names its unit,
  * wall stays available as context on a CPU breach, clearly labelled,
  * a wall ceiling still reports wall, so the fix does not invert the bug, and
  * a measurement whose unit differs from the bound's is REFUSED rather than
    recorded, so the substitution cannot happen silently again.
"""

from __future__ import annotations

import pytest

from safe_ci_dag_runner import scheduler as scheduler_module
from safe_ci_dag_runner.attribution import Culprit
from safe_ci_dag_runner.scheduler import Runner


class _Recorder:
    """Minimal stand-in for RunEvidence that keeps what was recorded."""

    def __init__(self) -> None:
        self.rows: list[tuple[str, dict[str, str]]] = []

    def record(self, event: str, fields: list[tuple[str, str]]) -> None:
        self.rows.append((event, dict(fields)))


def _capture(recorder: _Recorder | None, **kwargs: object) -> None:
    """Invoke the evidence capture with process inspection stubbed out.

    The unit check guards the call before any process is inspected, which is
    what lets these cases run without a live child.
    """
    scheduler = Runner.__new__(Runner)
    scheduler.evidence = recorder  # type: ignore[assignment]
    Runner._capture_termination_evidence(scheduler, **kwargs)  # type: ignore[arg-type]


def test_wall_measurement_against_a_cpu_bound_is_refused() -> None:
    """The exact substitution that caused the defect must not be recordable.

    This is the load-bearing case. If the guard is removed, a wall figure can
    once more be written into a CPU breach record and read as CPU-seconds.
    """
    with pytest.raises(ValueError) as excinfo:
        _capture(
            None,
            sink=None,
            step=None,
            proc=None,
            nonce="",
            event="cpu_timeout",
            limit_s=300,
            limit_unit="cpu_seconds",
            measured_s=354.587,
            measured_unit="wall_seconds",
            wall_elapsed_s=354.587,
        )
    message = str(excinfo.value)
    assert "cpu_timeout" in message
    assert "wall_seconds" in message
    assert "cpu_seconds" in message


def test_cpu_measurement_against_a_wall_bound_is_refused() -> None:
    """The mirror substitution is refused too, so the guard is not one-sided."""
    with pytest.raises(ValueError):
        _capture(
            None,
            sink=None,
            step=None,
            proc=None,
            nonce="",
            event="step_timeout",
            limit_s=900,
            limit_unit="wall_seconds",
            measured_s=308.57,
            measured_unit="cpu_seconds",
            wall_elapsed_s=354.7,
        )


class _Sink:
    """Stand-in stream whose attribution is empty, which is enough here."""

    def culprit(self) -> Culprit:
        return Culprit(test=None, how="stub", completed=0, last_completed=None)


class _Step:
    tag = "gate.manifest"


class _Proc:
    pid = 1


def _record(monkeypatch: pytest.MonkeyPatch, **kwargs: object) -> dict[str, str]:
    """Capture one breach with process inspection stubbed, return its fields."""
    monkeypatch.setattr(scheduler_module, "process_snapshot", lambda *_a, **_k: [])
    monkeypatch.setattr(
        scheduler_module, "bind_process_tests", lambda culprit, _obs: culprit
    )
    recorder = _Recorder()
    _capture(recorder, sink=_Sink(), step=_Step(), proc=_Proc(), nonce="", **kwargs)
    assert len(recorder.rows) == 1
    return recorder.rows[0][1]


def test_cpu_breach_records_the_cpu_seconds_it_compared_not_wall(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """A CPU kill must report the CPU figure the guard actually compared.

    The two numbers are deliberately far apart here, mirroring a real measured
    trip: about 308 CPU-seconds consumed against 354.7 wall. Recording wall as
    the compared quantity is the original defect.
    """
    fields = _record(
        monkeypatch,
        event="cpu_timeout",
        limit_s=300,
        limit_unit="cpu_seconds",
        measured_s=308.57,
        measured_unit="cpu_seconds",
        wall_elapsed_s=354.715,
    )
    assert fields["measured_s"] == "308.570"
    assert fields["measured_unit"] == "cpu_seconds"
    assert fields["limit_s"] == "300"
    assert fields["limit_unit"] == "cpu_seconds"
    # Wall stays available as context, but clearly labelled as wall.
    assert fields["wall_elapsed_s"] == "354.715"
    assert fields["measured_unit"] == fields["limit_unit"]


def test_wall_breach_still_reports_wall(monkeypatch: pytest.MonkeyPatch) -> None:
    """The wall ceiling is unchanged, so the fix does not invert the defect."""
    fields = _record(
        monkeypatch,
        event="step_timeout",
        limit_s=900,
        limit_unit="wall_seconds",
        measured_s=900.13,
        measured_unit="wall_seconds",
        wall_elapsed_s=900.13,
    )
    assert fields["measured_s"] == "900.130"
    assert fields["measured_unit"] == "wall_seconds"
    assert fields["limit_unit"] == "wall_seconds"
