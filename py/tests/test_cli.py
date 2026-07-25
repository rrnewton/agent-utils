"""Tests for the safe-ci-dag-runner CLI surface (in-process, stdlib capture)."""

from __future__ import annotations

import contextlib
import io
import subprocess
import sys
import tempfile
from pathlib import Path

from safe_ci_dag_runner import __version__
from safe_ci_dag_runner.cli import PROG, main

_DEMO = '{"steps": [{"group": "g", "job": "j", "cmd": "true", "deps": []}]}'


def _capture(args: list[str]) -> tuple[int, str, str]:
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        rc = main(args)
    return rc, out.getvalue(), err.getvalue()


def _demo_path(tmp: str) -> str:
    path = Path(tmp) / "dag.json"
    path.write_text(_DEMO, encoding="utf-8")
    return str(path)


def test_no_args_prints_help() -> None:
    rc, out, _ = _capture([])
    assert rc == 0
    assert PROG in out and "quickstart" in out


def test_quickstart_is_self_contained() -> None:
    rc, out, _ = _capture(["quickstart"])
    assert rc == 0
    for marker in ("Install", "Write a DAG", "run", "DAG schema", "Exit codes"):
        assert marker in out


def test_list_and_ascii() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        dag = _demo_path(tmp)
        rc, out, _ = _capture(["list", "--dag", dag])
        assert rc == 0 and "g.j" in out
        rc, out, _ = _capture(["ascii", "--dag", dag])
        assert rc == 0 and "layer 0:" in out and "g.j" in out


def test_dot_and_json() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        dag = _demo_path(tmp)
        rc, out, _ = _capture(["dot", "--dag", dag])
        assert rc == 0 and out.startswith("digraph")
        rc, out, _ = _capture(["json", "--dag", dag])
        assert rc == 0 and '"steps"' in out


def test_run_exit_codes() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        ok = Path(tmp) / "ok.json"
        ok.write_text('{"steps": [{"group": "g", "job": "j", "cmd": "true"}]}', encoding="utf-8")
        assert _capture(["run", "--dag", str(ok), "-q"])[0] == 0
        bad = Path(tmp) / "bad.json"
        bad.write_text('{"steps": [{"group": "g", "job": "j", "cmd": "false"}]}', encoding="utf-8")
        assert _capture(["run", "--dag", str(bad), "-q"])[0] == 1


def test_missing_and_malformed_dag_exit_2() -> None:
    assert _capture(["run", "--dag", "/nonexistent/nope.json", "-q"])[0] == 2
    with tempfile.TemporaryDirectory() as tmp:
        junk = Path(tmp) / "junk.json"
        junk.write_text("not json", encoding="utf-8")
        assert _capture(["list", "--dag", str(junk)])[0] == 2


def test_run_max_mem_exits_0() -> None:
    # --max-mem picks a memory-aware -j; a passing DAG still exits 0.
    with tempfile.TemporaryDirectory() as tmp:
        dag = _demo_path(tmp)
        rc, _, err = _capture(["run", "--max-mem", "8G", "--dag", dag, "-q"])
        assert rc == 0
        assert "--max-mem 8G" in err  # the sizing decision is surfaced


def test_run_jobs_overrides_max_mem() -> None:
    # When both --jobs and --max-mem are given, --jobs wins with a visible note.
    with tempfile.TemporaryDirectory() as tmp:
        dag = _demo_path(tmp)
        rc, _, err = _capture(["run", "--jobs", "2", "--max-mem", "8G", "--dag", dag, "-q"])
        assert rc == 0
        assert "--jobs=2 wins" in err


def test_run_perf_dir_writes_csv() -> None:
    # --perf-dir writes per-step + whole-run CSVs (the CsvMetricsSink path).
    with tempfile.TemporaryDirectory() as tmp:
        dag = _demo_path(tmp)
        perf = Path(tmp) / "perf"
        rc, _, err = _capture(["run", "--perf-dir", str(perf), "--dag", dag, "-q"])
        assert rc == 0
        csvs = list(perf.glob("*.csv"))
        assert csvs, "expected at least one perf CSV to be written"
        assert any(p.stat().st_size > 0 for p in csvs)
        assert "perf CSVs written under" in err


def test_version_via_module() -> None:
    # argparse --version exits(0); run as a subprocess so it doesn't kill the test process.
    result = subprocess.run(
        [sys.executable, "-m", "safe_ci_dag_runner", "--version"],
        capture_output=True,
        text=True,
        check=True,
    )
    assert result.stdout.strip() == f"{PROG} {__version__}"
