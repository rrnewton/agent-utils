"""Differential: wrkslotsd Python-CLI compatibility shell (#562 P10).

Routed as the wrkslots Rust edition (``WRKSLOTS_PYCOMPAT=1``, set by
the engine resolver), the binary accepts exact Python spellings:
leading globals move per-command and covered commands run under
their wrkslotsd names. The oracle is the Python ``status`` command on
the same fixture; unported spellings refuse by name.
"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path
from typing import cast

from wrkslots.tests.test_status_state_rust import _fixture, _root

from wrkslots.tests.rust_binary import rust_wrkslotsd


AU_ROOT = Path(__file__).resolve().parents[3]


def _compat(root: Path, *args: str) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["WRKSLOTS_PYCOMPAT"] = "1"
    return subprocess.run(
        [str(rust_wrkslotsd()), "--project-root", str(root), *args],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _python(root: Path, *args: str) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["PYTHONPATH"] = str(AU_ROOT / "py")
    return subprocess.run(
        ["python3", "-m", "wrkslots", "--project-root", str(root), "status", *args],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _normalized(document: dict[str, object]) -> dict[str, object]:
    active = cast(list[dict[str, object]], document["active"])
    for record in active:
        age = cast(int, record.pop("heartbeat_age_seconds"))
        assert age >= 0
    return document


def test_compat_status_json_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    python_result = _python(root, "--format", "json")
    compat_result = _compat(root, "status", "--format", "json")
    assert python_result.returncode == 0, python_result.stderr
    assert compat_result.returncode == 0, compat_result.stderr
    python_document = cast(dict[str, object], json.loads(python_result.stdout))
    compat_document = cast(dict[str, object], json.loads(compat_result.stdout))
    assert _normalized(compat_document) == _normalized(python_document)


def test_compat_quickstart_matches_native(tmp_path: Path) -> None:
    root = _root(tmp_path)
    native = subprocess.run(
        [str(rust_wrkslotsd()), "quickstart"],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )
    compat = _compat(root, "quickstart")
    assert native.returncode == 0, native.stderr
    assert compat.returncode == 0, compat.stderr
    assert compat.stdout == native.stdout


def test_compat_unported_refuses_by_name(tmp_path: Path) -> None:
    root = _root(tmp_path)
    result = _compat(root, "init", ".")
    assert result.returncode == 1
    assert "'init' stays with the Python edition" in result.stderr


def test_compat_status_verbose_refuses(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    result = _compat(root, "status", "--verbose")
    assert result.returncode == 1
    assert "status --verbose" in result.stderr


def test_compat_abandon_prepared_refuses_by_name(tmp_path: Path) -> None:
    """#1374: abandoning an ownerless journal stays with the Python edition."""

    root = _root(tmp_path)
    _fixture(root)
    result = _compat(
        root, "recover-ownerless-agent-worktree", "worktrees/orphan", "--repository", "repo",
        "--head", "0" * 40, "--branch", "agent/orphan", "--remote", "origin",
        "--remote-url-sha256", "0" * 64, "--abandon-prepared",
    )
    assert result.returncode == 1
    assert (
        "recover-ownerless-agent-worktree --abandon-prepared stays with the Python edition"
        in result.stderr
    )
