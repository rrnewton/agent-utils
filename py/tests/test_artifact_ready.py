"""Artifact-ready early release: scheduler behavior plus load validation.

Mirrors rs/dagrun/tests/artifact_ready_smoke.rs. A dependent declaring
``artifact_deps`` starts when the producer signals, while the producer
keeps running; completion dependents still wait for exit. Failure
semantics are fail-closed.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from dagrun.io import DagJsonError, dag_from_json, dag_to_json
from dagrun.model import ArtifactDep, DagConfig, PublishedArtifact, Step
from dagrun.scheduler import run_dag


def _producer(tmp_path: Path, body: str) -> Step:
    return Step(
        "build",
        "P",
        "",
        body,
        publishes=[PublishedArtifact("x")],
    )


def _dependent(tmp_path: Path) -> Step:
    return Step(
        "e2e",
        "D",
        "",
        f"date +%s.%N > {tmp_path}/d_start; sleep 1; date +%s.%N > {tmp_path}/d_end",
        artifact_deps=[ArtifactDep("build.P", "x")],
    )


def _control(tmp_path: Path) -> Step:
    return Step(
        "gate",
        "C",
        "",
        f"date +%s.%N > {tmp_path}/c_start",
        deps=["build.P"],
    )


def _stamp(tmp_path: Path, name: str) -> float:
    return float((tmp_path / name).read_text(encoding="utf-8").strip())


_SIGNAL = (
    'printf x > "$DAGRUN_ARTIFACT_READY_DIR/build.P/x.tmp"; '
    'mv "$DAGRUN_ARTIFACT_READY_DIR/build.P/x.tmp" '
    '"$DAGRUN_ARTIFACT_READY_DIR/build.P/x"'
)


def test_dependent_releases_at_signal_while_producer_keeps_running(
    tmp_path: Path,
) -> None:
    producer = _producer(
        tmp_path,
        f"date +%s.%N > {tmp_path}/p_start; sleep 2; {_SIGNAL}; "
        f"sleep 6; date +%s.%N > {tmp_path}/p_end",
    )
    result = run_dag(
        DagConfig(steps=(producer, _dependent(tmp_path), _control(tmp_path))),
        jobs=4,
        verbosity=0,
    )
    assert result.ok
    p = _stamp(tmp_path, "p_start")
    assert 1.5 < _stamp(tmp_path, "d_start") - p < 4.0
    assert _stamp(tmp_path, "d_end") < _stamp(tmp_path, "p_end")
    assert _stamp(tmp_path, "c_start") >= _stamp(tmp_path, "p_end") - 0.05


def test_producer_failure_after_signal_discards_released_dependent(
    tmp_path: Path,
) -> None:
    producer = _producer(
        tmp_path, f"sleep 2; {_SIGNAL}; sleep 6; exit 1"
    )
    result = run_dag(
        DagConfig(steps=(producer, _dependent(tmp_path), _control(tmp_path))),
        jobs=4,
        verbosity=0,
    )
    assert not result.ok
    assert (tmp_path / "d_start").exists(), "D should have been released"
    assert not (tmp_path / "c_start").exists(), "C must never start"
    by_tag = {outcome.tag: outcome for outcome in result.outcomes}
    assert not by_tag["build.P"].ok
    assert not by_tag["e2e.D"].ok, "released dependent keeps no credit"
    assert "build.P" in by_tag["e2e.D"].reason
    assert "gate.C" in result.skipped


def test_missing_signal_fails_closed_and_dependent_never_starts(
    tmp_path: Path,
) -> None:
    producer = _producer(tmp_path, "sleep 1")
    result = run_dag(
        DagConfig(steps=(producer, _dependent(tmp_path))),
        jobs=4,
        verbosity=0,
    )
    assert not result.ok
    assert not (tmp_path / "d_start").exists(), "D must never start"
    by_tag = {outcome.tag: outcome for outcome in result.outcomes}
    assert not by_tag["build.P"].ok
    assert "ARTIFACT NOT SIGNALLED" in by_tag["build.P"].reason
    assert "x" in by_tag["build.P"].reason


def test_artifact_graph_validation_and_round_trip() -> None:
    doc = (
        '{"steps":['
        '{"group":"build","job":"P","cmd":"true","publishes":[{"name":"x"}]},'
        '{"group":"e2e","job":"D","cmd":"true",'
        '"artifact_deps":[{"step":"build.P","artifact":"x"}]}]}'
    )
    cfg = dag_from_json(doc)
    assert dag_from_json(dag_to_json(cfg)).steps[1].artifact_deps == [
        ArtifactDep("build.P", "x")
    ]

    for bad, needle in [
        (
            '{"steps":[{"group":"e2e","job":"D","cmd":"true",'
            '"artifact_deps":[{"step":"build.P","artifact":"x"}]}]}',
            "which no step declares",
        ),
        (
            '{"steps":[{"group":"build","job":"P","cmd":"true",'
            '"publishes":[{"name":"x"}]},{"group":"e2e","job":"D","cmd":"true",'
            '"artifact_deps":[{"step":"build.P","artifact":"y"}]}]}',
            "does not publish",
        ),
        (
            '{"steps":[{"group":"a","job":"one","cmd":"true",'
            '"publishes":[{"name":"x"}],"deps":["b.two"]},'
            '{"group":"b","job":"two","cmd":"true",'
            '"artifact_deps":[{"step":"a.one","artifact":"x"}]}]}',
            "dependency cycle",
        ),
        (
            '{"steps":[{"group":"build","job":"P","cmd":"true",'
            '"publishes":[{"name":"x"},{"name":"x"}]}]}',
            "more than once",
        ),
    ]:
        with pytest.raises(DagJsonError) as excinfo:
            dag_from_json(bad)
        assert needle in str(excinfo.value)
