"""Fast bounded doctor and per-phase timings (#1706).

`doctor --quick` is the periodic-gate mode: it reads only the
registry snapshot (under the shared lock), never spawns a subprocess,
and names every check it skips.  The full doctor reports per-phase
timings and its subprocess count in --json, and its slow checks run
outside the lock, so another command proceeds meanwhile.  Waits are
on conditions; timeouts only detect a hang.
"""

from __future__ import annotations

import json
import threading
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import create, make_project


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")


def test_quick_never_runs_the_slow_checks(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0

    def forbidden(*args: object, **kwargs: object) -> object:
        raise AssertionError("quick mode ran a slow check")

    monkeypatch.setattr(cli, "_shared_submodule_doctor_findings", forbidden)
    monkeypatch.setattr(cli, "_worktree_config_doctor_findings", forbidden)
    monkeypatch.setattr(cli, "_registry_storage_inconsistencies", forbidden)
    monkeypatch.setattr(cli, "_owner_liveness_states", forbidden)

    code = cli.main(
        ["--project-root", str(project), "doctor", "--quick",
         "--format", "json"]
    )
    payload = json.loads(capsys.readouterr().out)
    assert code == 0
    assert payload["quick"] is True
    assert payload["subprocesses"] == 0
    assert payload["counts_by_kind"] == {}
    assert payload["interrupted_journals"] == []
    assert payload["slowest_check"] in payload["timings"]
    skipped = " ".join(payload["skipped"])
    assert "shared-submodule" in skipped
    assert "liveness probes" in skipped
    assert "storage census" in skipped
    (row,) = payload["active"]
    assert row["slot"] == "slot01"


def test_quick_human_names_what_it_skips(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    code = cli.main(["--project-root", str(project), "doctor", "--quick"])
    out = capsys.readouterr().out
    assert code == 0
    assert "quick=true" in out
    assert out.startswith("project=")
    assert "SKIPPED:" in out
    assert "subprocesses=0" in out


def test_full_doctor_reports_phase_timings(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    code = cli.main(
        ["--project-root", str(project), "doctor", "--format", "json"]
    )
    payload = json.loads(capsys.readouterr().out)
    assert code == 0
    timings = payload["timings"]
    for phase in (
        "snapshot",
        "storage_census",
        "owner_liveness",
        "shared_submodule",
        "worktree_config",
        "total",
    ):
        assert phase in timings, phase
        assert timings[phase] >= 0
    assert payload["subprocesses"] > 0
    assert payload["counts_by_kind"] == {}


def test_full_doctor_slow_checks_hold_no_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    assert (
        create(
            project, slot="slot02", agent="codex-2", branch="codex/task2"
        ).returncode
        == 0
    )
    entered = threading.Event()
    release = threading.Event()
    real = cli._shared_submodule_doctor_findings

    def blocking(
        config: cli.Config, states: object
    ) -> list[dict[str, object]]:
        entered.set()
        assert release.wait(timeout=60)
        return real(config, states)  # type: ignore[arg-type]

    monkeypatch.setattr(cli, "_shared_submodule_doctor_findings", blocking)
    outcome: dict[str, object] = {}

    def run_doctor() -> None:
        outcome["returncode"] = cli.main(
            ["--project-root", str(project), "doctor"]
        )

    worker = threading.Thread(target=run_doctor, daemon=True)
    worker.start()
    try:
        assert entered.wait(timeout=60)
        # Doctor is inside its slow checks; the lock is free, so an
        # unrelated mutation proceeds.
        code = cli.main(
            [
                "--project-root", str(project), "hold", "slot02",
                "--reason", "keep",
            ]
        )
        assert code == 0
    finally:
        release.set()
    worker.join(timeout=60)
    assert not worker.is_alive()
    assert outcome["returncode"] == 0


def test_quick_json_matches_rust(tmp_path: Path) -> None:
    """Rust doctor --quick carries the same fields (#1706 parity)."""
    import subprocess

    from wrkslots.tests.rust_binary import rust_wrkslotsd
    from wrkslots.tests.test_doctor_state_rust import _fixture, _root

    root = _root(tmp_path)
    _fixture(root)
    env_python = [
        "python3", "-m", "wrkslots", "--project-root", str(root),
        "doctor", "--quick", "--format", "json",
    ]
    import os

    env = dict(os.environ)
    env["PYTHONPATH"] = str(Path(__file__).resolve().parents[2])
    python_result = subprocess.run(
        env_python, capture_output=True, text=True, check=False, env=env,
        timeout=900,
    )
    rust_result = subprocess.run(
        [str(rust_wrkslotsd()), "doctor-text", "--quick", "--root",
         str(root), "--format", "json"],
        capture_output=True, text=True, check=False, timeout=900,
    )
    assert python_result.returncode == 0, python_result.stderr
    assert rust_result.returncode == 0, rust_result.stderr
    python_doc = json.loads(python_result.stdout)
    rust_doc = json.loads(rust_result.stdout)
    for doc in (python_doc, rust_doc):
        doc.pop("timings", None)
        doc.pop("subprocesses", None)
        for record in doc["active"]:
            record.pop("heartbeat_age_seconds", None)
    assert rust_doc == python_doc
