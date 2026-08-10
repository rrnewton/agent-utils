from __future__ import annotations

import json
import os
from pathlib import Path

from safe_ci_dag_runner.attribution import (
    Culprit,
    ProcessObservation,
    RunEvidence,
    StepStream,
    bind_process_tests,
    recognize,
)


def test_recognizes_shared_harness_boundaries() -> None:
    start = recognize("test suite::hang ... ")
    end = recognize("test suite::ok ... ok")
    pytest_end = recognize("tests/test_x.py::test_y PASSED")
    tap_end = recognize("not ok 12 - rejects input")
    assert start is not None and (start.kind, start.name) == ("start", "suite::hang")
    assert end is not None and (end.kind, end.name, end.verdict) == (
        "end",
        "suite::ok",
        "ok",
    )
    assert pytest_end is not None and pytest_end.kind == "end"
    assert tap_end is not None and tap_end.verdict == "not ok"


def test_unterminated_tail_names_the_culprit() -> None:
    stream = StepStream("tests.suite", None)
    stream.ingest(b"test suite::alpha ... ok\ntest suite::gamma ... ")
    culprit = stream.culprit()
    assert culprit.test == "suite::gamma"
    assert culprit.completed == 1
    assert culprit.last_completed == "suite::alpha"


def test_parallel_boundaries_report_all_live_tests_with_elapsed_time() -> None:
    stream = StepStream("tests.suite", None)
    stream.ingest(b"##TEST-START suite::older\n")
    stream.ingest(b"##TEST-START suite::newer\n")
    culprit = stream.culprit()
    assert culprit.test == "suite::older"
    assert [test.name for test in culprit.in_flight] == ["suite::older", "suite::newer"]
    assert all(test.elapsed_s >= 0 for test in culprit.in_flight)
    assert "likely culprit test suite::older" in culprit.describe()


def test_process_binding_uses_exact_test_identity_and_refuses_unbound_rows() -> None:
    unknown = Culprit(None, "no boundaries", 0, None)
    unbound = ProcessObservation(10, 1, "shared-test-binary", 3.0, 0.0, "wall-stalled")
    assert bind_process_tests(unknown, (unbound,)).test is None

    bound = ProcessObservation(
        11,
        1,
        "test-bin --exact suite::hang",
        3.0,
        0.0,
        "wall-stalled",
        "suite::hang",
        "libtest --exact process argv",
    )
    culprit = bind_process_tests(unknown, (unbound, bound))
    assert culprit.test == "suite::hang"
    assert culprit.in_flight[0].basis == "libtest --exact process argv"


def test_evidence_is_private_and_records_boundaries(tmp_path: Path) -> None:
    directory = tmp_path / "evidence"
    evidence = RunEvidence.open(directory)
    assert evidence is not None
    stream = StepStream("g:j", evidence)
    stream.ingest(b"##TEST-START case\n##TEST-END case PASSED\n")
    evidence.record("done", [("ok", "true")])

    assert directory.stat().st_mode & 0o777 == 0o700
    assert (directory / "journal.jsonl").stat().st_mode & 0o777 == 0o600
    assert (directory / "g~3aj.log").stat().st_mode & 0o777 == 0o600
    records = [json.loads(line) for line in (directory / "journal.jsonl").read_text().splitlines()]
    assert [record["event"] for record in records] == ["test_start", "test_end", "done"]


def test_evidence_refuses_symlinks_and_fifos_without_blocking(tmp_path: Path) -> None:
    target = tmp_path / "target"
    target.mkdir(mode=0o700)
    link = tmp_path / "link"
    link.symlink_to(target, target_is_directory=True)
    assert RunEvidence.open(link) is None

    journal_dir = tmp_path / "journal-fifo"
    journal_dir.mkdir(mode=0o700)
    os.mkfifo(journal_dir / "journal.jsonl", mode=0o600)
    assert RunEvidence.open(journal_dir) is None

    step_dir = tmp_path / "step-fifo"
    evidence = RunEvidence.open(step_dir)
    assert evidence is not None
    os.mkfifo(step_dir / "g~3aj.log", mode=0o600)
    assert evidence.open_step_log("g:j") is None


def test_evidence_rejects_overlong_log_name(tmp_path: Path) -> None:
    evidence = RunEvidence.open(tmp_path / "evidence")
    assert evidence is not None
    assert evidence.open_step_log("/" * 100) is None
