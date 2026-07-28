"""Tests for the pr-landing-planner CLI surface (in-process, stdlib capture)."""

from __future__ import annotations

import contextlib
import io
import json
import subprocess
import sys
from pathlib import Path

from pr_landing_planner import __version__
from pr_landing_planner.cli import PROG, _load_userguide, main


def _capture(args: list[str]) -> tuple[int, str, str]:
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        rc = main(args)
    return rc, out.getvalue(), err.getvalue()


def _example(name: str) -> str:
    return str(Path(__file__).resolve().parent.parent / "pr_landing_planner" / "examples" / name)


DEMO = _example("pr-landing-demo.yaml")
FLAKY = _example("flaky-signatures.yaml")


def test_no_args_prints_help() -> None:
    rc, out, _ = _capture([])
    assert rc == 0
    assert PROG in out and "quickstart" in out


def test_quickstart_is_self_contained() -> None:
    rc, out, _ = _capture(["quickstart"])
    assert rc == 0
    for marker in ("five red classifications", "Per-PR actions", "Output formats", "Exit codes", "Demo fixture"):
        assert marker in out


def test_quickstart_emit_demo_is_loadable() -> None:
    rc, out, _ = _capture(["quickstart", "--emit-demo"])
    assert rc == 0 and "prs:" in out


def test_userguide_prints_embedded_guide() -> None:
    rc, out, _ = _capture(["--userguide"])
    assert rc == 0
    assert out == _load_userguide()
    assert len(out) > 3000
    assert "pr-landing-planner" in out


def test_plan_human_all_classes() -> None:
    rc, out, _ = _capture(["plan", "--fixture", DEMO, "--flaky-signatures", FLAKY])
    assert rc == 0
    assert "land-now" in out
    assert "rebase-then-land" in out
    assert "refire-stale-gate" in out
    assert "refire-ci" in out
    assert "escalate-runner-outage" in out
    assert "SYSTEMIC RUNNER OUTAGE" in out
    assert "Parallel-safe groups" in out


def test_plan_json_is_valid_and_has_schema() -> None:
    rc, out, _ = _capture(["plan", "--fixture", DEMO, "--flaky-signatures", FLAKY, "--format", "json"])
    assert rc == 0
    obj = json.loads(out)
    assert obj["repository"] == "OWNER/NAME"
    assert set(obj["plan"]) >= {"parallel_safe_groups", "land_now", "order", "per_pr_actions"}
    assert 1043 in obj["plan"]["land_now"]
    assert 1049 in obj["diagnostics"]["flaky_reds"]
    assert obj["diagnostics"]["outage_suspected"] is True
    # Deterministic: identical bytes on a second run.
    _, out2, _ = _capture(["plan", "--fixture", DEMO, "--flaky-signatures", FLAKY, "--format", "json"])
    assert out == out2


def test_plan_actions_has_capturable_summary_and_loud_lines() -> None:
    rc, out, _ = _capture(["plan", "--fixture", DEMO, "--flaky-signatures", FLAKY, "--format", "actions"])
    assert rc == 0
    lines = out.splitlines()
    # Bare key=value summary lines (tick-hub `capture: true` lifts these).
    assert "land_now=1" in lines
    assert "outage=1" in lines
    assert any(line.startswith("stale_gates=") for line in lines)
    # Loud diagnostics + per-PR ACTION lines parseable by leading token.
    assert any(line.startswith("ERROR: ci-hosted-runner-outage-systemic") for line in lines)
    assert any(line.startswith("ACTION: land-now pr=1043") for line in lines)
    assert any(line.startswith("NOTE: evaluate-once-race pr=1050") for line in lines)


def test_graph_view() -> None:
    rc, out, _ = _capture(["graph", "--fixture", DEMO])
    assert rc == 0
    assert "Real conflicts" in out and "Stacks" in out
    rc, out, _ = _capture(["graph", "--fixture", DEMO, "--format", "json"])
    obj = json.loads(out)
    assert "conflict_edges" in obj and "plan" not in obj


def test_status_view_and_threshold_warning() -> None:
    rc, out, _ = _capture(["status", "--fixture", DEMO, "--warn-threshold", "3"])
    assert rc == 0
    assert "Open PR health" in out
    assert "WARNING" in out  # 9 open PRs exceeds 3
    rc, out, _ = _capture(["status", "--fixture", DEMO, "--format", "json"])
    obj = json.loads(out)
    assert obj["summary"]["open"] == 9


def test_missing_fixture_exits_2() -> None:
    rc, _, err = _capture(["plan", "--fixture", "/nonexistent/nope.yaml"])
    assert rc == 2
    assert PROG in err


def test_version_via_module() -> None:
    py_root = Path(__file__).resolve().parent.parent
    result = subprocess.run(
        [sys.executable, "-m", "pr_landing_planner", "--version"],
        capture_output=True,
        text=True,
        check=True,
        cwd=str(py_root),
    )
    assert result.stdout.strip() == f"{PROG} {__version__}"
