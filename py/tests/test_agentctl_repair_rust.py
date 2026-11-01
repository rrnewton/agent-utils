"""Differential: Rust `agentctl repair` against the Python operation.

#545 rust-primary-agentctl-chat-readers: repair (presentation recreation
for a live headless runner) at parity. Both editions repair copies of
the same fixture with a fake `tmux` on PATH and a live sleeper as the
runner; they must print the same document and leave the same state.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path
from typing import cast

from agentctl import sessions as sessions_module
from tests.test_agentctl_sessions import setup


AU_ROOT = Path(__file__).resolve().parents[2]

FAKE_TMUX = r"""#!/bin/sh
# Fake tmux: windows are lines in $FAKE_TMUX_STATE, calls logged to $FAKE_TMUX_LOG.
state="$FAKE_TMUX_STATE"
echo "$@" >> "$FAKE_TMUX_LOG"
cmd="$1"; shift
case "$cmd" in
  list-windows)
    cat "$state" 2>/dev/null || true
    ;;
  has-session)
    grep -qx "__session__" "$state" 2>/dev/null
    ;;
  new-session)
    echo "__session__" >> "$state"
    while [ $# -gt 0 ]; do
      if [ "$1" = "-n" ]; then echo "$2" >> "$state"; fi
      shift
    done
    ;;
  new-window)
    while [ $# -gt 0 ]; do
      if [ "$1" = "-n" ]; then echo "$2" >> "$state"; fi
      shift
    done
    ;;
  list-panes)
    : # no panes: the transcript-view path
    ;;
  break-pane)
    exit 1
    ;;
esac
exit 0
"""


def _start_time(pid: int) -> int:
    text = Path(f"/proc/{pid}/stat").read_text()
    return int(text.rsplit(")", 1)[1].split()[19])


def _fixture(tmp_path: Path, monkeypatch: object) -> tuple[Path, Path, subprocess.Popen[str]]:
    sessions, _fake, _calls = setup(tmp_path, monkeypatch)  # type: ignore[arg-type]
    sessions.start_session("worker", cwd=str(tmp_path))
    runner = subprocess.Popen(["sleep", "300"], text=True)
    runtime = tmp_path / "runtime"
    (runtime / "state" / "worker" / "inbox").mkdir(parents=True)
    (runtime / "registry.json").write_text(json.dumps([{
        "name": "worker", "harness": "codex", "backend": "tmux",
        "tmux_target": "subagents:worker", "cwd": str(tmp_path), "model": None,
        "session_id": "sess", "status": "idle", "runner_pid": runner.pid,
        "runner_started_at": _start_time(runner.pid), "next_seq": 1,
        "created_at": "2026-01-01T00:00:00Z", "last_turn_at": None,
        "mode": "headless", "presentation_pane": None,
        "codex_bypass_permissions": False, "harness_args": [],
    }]))
    record = sessions.get("worker")
    record.adapter = "turn-runner"
    record.mode = "headless"
    record.runtime_home = str(runtime)
    sessions._save(record)
    return Path(sessions.registry), runtime, runner


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


def _run(launcher: Path, registry: Path, fake_bin: Path, state: Path, log: Path) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["PATH"] = f"{fake_bin}:{env['PATH']}"
    env["FAKE_TMUX_STATE"] = str(state)
    env["FAKE_TMUX_LOG"] = str(log)
    return subprocess.run(
        [str(launcher), "--registry", str(registry), "repair", "worker"],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def test_rust_repair_matches_python(tmp_path: Path, monkeypatch: object) -> None:
    registry, runtime, runner = _fixture(tmp_path, monkeypatch)
    try:
        fake_bin = tmp_path / "fake-bin"
        fake_bin.mkdir()
        script = fake_bin / "tmux"
        script.write_text(FAKE_TMUX)
        script.chmod(0o755)

        python_registry, python_runtime = _copy_fixture(registry, runtime, tmp_path / "py-copy")
        rust_registry, rust_runtime = _copy_fixture(registry, runtime, tmp_path / "rs-copy")
        py_state = tmp_path / "py-tmux-state"
        rs_state = tmp_path / "rs-tmux-state"

        python_result = _run(AU_ROOT / "py" / "bin" / "agentctl", python_registry, fake_bin, py_state, tmp_path / "py.log")
        rust_result = _run(AU_ROOT / "rs" / "bin" / "agentctl", rust_registry, fake_bin, rs_state, tmp_path / "rs.log")
        assert python_result.returncode == 0, python_result.stderr
        assert rust_result.returncode == 0, rust_result.stderr
        python_document = json.loads(python_result.stdout)
        rust_document = json.loads(rust_result.stdout)

        def normalized(document: dict[str, object], runtime_dir: Path) -> dict[str, object]:
            text = json.dumps(document, sort_keys=True).replace(str(runtime_dir), "<runtime>")
            return cast(dict[str, object], json.loads(text))

        assert normalized(rust_document, rust_runtime) == normalized(python_document, python_runtime)
        assert python_document["result"]["mode"] == "transcript_view"
        # Both editions recorded the same tmux calls and registry target.
        assert (tmp_path / "py.log").read_text().replace(str(python_runtime), "<runtime>") == (tmp_path / "rs.log").read_text().replace(str(rust_runtime), "<runtime>")
        py_rows = json.loads((python_runtime / "registry.json").read_text())
        rs_rows = json.loads((rust_runtime / "registry.json").read_text())
        assert rs_rows == py_rows
    finally:
        runner.terminate()
        runner.wait(timeout=10)


def test_rust_repair_refuses_reused_pid_like_python(tmp_path: Path, monkeypatch: object) -> None:
    registry, runtime, runner = _fixture(tmp_path, monkeypatch)
    try:
        # A live pid whose recorded start time is wrong is a reused pid.
        row_path = runtime / "registry.json"
        rows = json.loads(row_path.read_text())
        rows[0]["runner_started_at"] = 1
        row_path.write_text(json.dumps(rows))
        fake_bin = tmp_path / "fake-bin"
        fake_bin.mkdir(exist_ok=True)
        script = fake_bin / "tmux"
        script.write_text(FAKE_TMUX)
        script.chmod(0o755)
        python_registry, python_runtime = _copy_fixture(registry, runtime, tmp_path / "py-copy2")
        rust_registry, rust_runtime = _copy_fixture(registry, runtime, tmp_path / "rs-copy2")
        # The copies must carry the tampered row too.
        for copy_runtime in (python_runtime, rust_runtime):
            copy_rows = json.loads((copy_runtime / "registry.json").read_text())
            copy_rows[0]["runner_started_at"] = 1
            (copy_runtime / "registry.json").write_text(json.dumps(copy_rows))
        python_result = _run(AU_ROOT / "py" / "bin" / "agentctl", python_registry, fake_bin, tmp_path / "py2-state", tmp_path / "py2.log")
        rust_result = _run(AU_ROOT / "rs" / "bin" / "agentctl", rust_registry, fake_bin, tmp_path / "rs2-state", tmp_path / "rs2.log")
        assert python_result.returncode != 0
        assert rust_result.returncode == python_result.returncode, rust_result.stderr
        assert "is not alive or was reused" in rust_result.stderr
        assert "is not alive or was reused" in python_result.stderr
    finally:
        runner.terminate()
        runner.wait(timeout=10)
