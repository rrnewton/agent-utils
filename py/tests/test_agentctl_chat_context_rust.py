"""Differential: Rust `agentctl chat context` over the adapter transport.

#545 rust-primary-agentctl-chat-readers. The fixture (saved bridge state plus
a recording command adapter) is the one the Python context CLI tests use;
both editions read two pages through it and must print the same documents
and make the same adapter calls.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

from tests.test_agentctl_chat_context_cli import _KEY, _saved


AU_ROOT = Path(__file__).resolve().parents[2]
PYTHON_LAUNCHER = AU_ROOT / "py" / "bin" / "agentctl"
RUST_LAUNCHER = AU_ROOT / "rs" / "bin" / "agentctl"


def _run(launcher: Path, state_flag: str, state: Path, cursor: str | None) -> dict[str, object]:
    command = [str(launcher), "chat", "context", state_flag, str(state), "--request", _KEY[:12]]
    if cursor is not None:
        command += ["--cursor", cursor]
    result = subprocess.run(
        command, cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900
    )
    assert result.returncode == 0, result.stderr
    parsed: dict[str, object] = json.loads(result.stdout)
    return parsed


def test_rust_context_matches_python_over_command_adapter(tmp_path: Path) -> None:
    root = tmp_path / "py"; root.mkdir()
    state, transcript = _saved(root)
    first = _run(PYTHON_LAUNCHER, "--state", state, None)
    second = _run(PYTHON_LAUNCHER, "--state", state, str(first["cursor"]))
    python_calls = [json.loads(line) for line in transcript.read_text().splitlines()]
    transcript.write_text("")

    first_rust = _run(RUST_LAUNCHER, "--bridge-state", state, None)
    second_rust = _run(RUST_LAUNCHER, "--bridge-state", state, str(first_rust["cursor"]))
    rust_calls = [json.loads(line) for line in transcript.read_text().splitlines()]

    assert first_rust == first
    assert second_rust == second
    assert rust_calls == python_calls


def test_rust_context_refuses_without_adapter(tmp_path: Path) -> None:
    root = tmp_path / "py"; root.mkdir()
    state, _transcript = _saved(root)
    bridge = json.loads((state / "bridge.json").read_text())
    bridge["config"]["transport_command"] = []
    (state / "bridge.json").write_text(json.dumps(bridge))
    result = subprocess.run(
        [str(RUST_LAUNCHER), "chat", "context", "--bridge-state", str(state),
         "--request", _KEY[:12]],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )
    assert result.returncode == 1
    assert "not implemented yet" in result.stderr
    assert "DAGRUN_ENGINE=python" in result.stderr
