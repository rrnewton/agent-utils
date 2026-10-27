"""Differential: Rust `agentctl reset` against the Python operation.

#545 rust-primary-agentctl-chat-readers (coordinator decision: native reset
now; migrate/repair fold into the herdr-subagents port). The canonical
turn-runner record is built with the Python sessions fixture machinery; the
foreign runtime home is a hand-written registry row, which is all `reset`
touches. Both editions reset copies of the same fixture and must print the
same document and leave the same canonical and foreign state.
"""

from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

from agentctl import sessions as sessions_module
from tests.test_agentctl_sessions import setup


AU_ROOT = Path(__file__).resolve().parents[2]


def _fixture(tmp_path: Path, monkeypatch: object) -> tuple[Path, Path]:
    sessions, _fake, _calls = setup(tmp_path, monkeypatch)  # type: ignore[arg-type]
    sessions.start_session("worker", cwd=str(tmp_path))
    runtime = tmp_path / "runtime"
    (runtime / "state" / "worker" / "inbox").mkdir(parents=True)
    (runtime / "registry.json").write_text(json.dumps([{
        "name": "worker", "harness": "codex", "backend": "tmux",
        "tmux_target": "s:1", "cwd": str(tmp_path), "model": None,
        "session_id": "old-session", "status": "idle", "runner_pid": 12,
        "runner_started_at": None, "next_seq": 1,
        "created_at": "2026-01-01T00:00:00Z", "last_turn_at": None,
        "mode": "headless", "presentation_pane": None,
        "codex_bypass_permissions": False, "harness_args": [],
    }]))
    record = sessions.get("worker")
    record.adapter = "turn-runner"
    record.mode = "headless"
    record.runtime_home = str(runtime)
    sessions._save(record)
    registry = Path(sessions.registry)
    return registry, runtime


def _copy_fixture(source: Path, runtime: Path, target: Path) -> tuple[Path, Path]:
    registry = target / "registry"
    shutil.copytree(source, registry)
    new_runtime = target / "runtime"
    shutil.copytree(runtime, new_runtime)
    record_path = registry / "agents" / "worker" / "agent.json"
    document = json.loads(record_path.read_text())
    document["runtime_home"] = str(new_runtime)
    record_path.write_text(json.dumps(document))
    return registry, new_runtime


def _run(launcher: Path, registry: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(launcher), "--registry", str(registry), "reset", "worker"],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def test_rust_reset_matches_python(tmp_path: Path, monkeypatch: object) -> None:
    registry, runtime = _fixture(tmp_path, monkeypatch)

    python_registry, python_runtime = _copy_fixture(registry, runtime, tmp_path / "py-copy")
    rust_registry, rust_runtime = _copy_fixture(registry, runtime, tmp_path / "rs-copy")

    python_result = _run(AU_ROOT / "py" / "bin" / "agentctl", python_registry)
    rust_result = _run(AU_ROOT / "rs" / "bin" / "agentctl", rust_registry)
    assert python_result.returncode == 0, python_result.stderr
    assert rust_result.returncode == 0, rust_result.stderr
    python_document = json.loads(python_result.stdout)
    rust_document = json.loads(rust_result.stdout)
    python_document["result"]["transcript"] = python_document["result"]["transcript"].replace(
        str(python_runtime), "<runtime>")
    rust_document["result"]["transcript"] = rust_document["result"]["transcript"].replace(
        str(rust_runtime), "<runtime>")
    assert rust_document == python_document

    python_record = json.loads((python_registry / "agents" / "worker" / "agent.json").read_text())
    rust_record = json.loads((rust_registry / "agents" / "worker" / "agent.json").read_text())
    for key in ("mode", "backend", "session_value", "native_session", "pane_id"):
        assert rust_record.get(key) == python_record.get(key), key
    python_rows = json.loads((python_runtime / "registry.json").read_text())
    rust_rows = json.loads((rust_runtime / "registry.json").read_text())
    assert rust_rows == python_rows
    assert (rust_runtime / "state" / "worker" / "transcript.log").read_text() == "=== CONTEXT RESET ===\n"
