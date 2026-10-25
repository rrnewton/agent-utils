"""Engine-selection contract for the tracked ``common/bin`` dispatcher.

Paired tools default to the Rust launcher; Python-only tools default to the
Python entrypoint. ``DAGRUN_ENGINE`` and ``AGENT_UTILS_ENGINE`` are explicit
overrides (``DAGRUN_ENGINE`` wins when both are set), and an explicit engine
request for a tool without that edition refuses instead of falling back.

The sandbox uses stub launchers, so these tests pin the resolver's dispatch
decision without building any Rust.
"""

from __future__ import annotations

import os
import shutil
import stat
import subprocess
from pathlib import Path


AU_ROOT = Path(__file__).resolve().parents[2]
RESOLVER = AU_ROOT / "common" / "bin" / "engine-resolver"

PAIRED = "tick-hub"
PYTHON_ONLY = "wrkslots"


def _chmod_x(path: Path) -> None:
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def _build_sandbox(tmp_path: Path) -> Path:
    root = tmp_path / "au"
    common_bin = root / "common" / "bin"
    python_bin = root / "py" / "bin"
    rust_bin = root / "rs" / "bin"
    for directory in (common_bin, python_bin, rust_bin):
        directory.mkdir(parents=True)
    shutil.copy2(RESOLVER, common_bin / "engine-resolver")
    _chmod_x(common_bin / "engine-resolver")
    for tool in (PAIRED, PYTHON_ONLY):
        (common_bin / tool).symlink_to("engine-resolver")
        stub = python_bin / tool
        stub.write_text(f"print('PY-{tool}')\n")
        _chmod_x(stub)
    rust_stub = rust_bin / PAIRED
    rust_stub.write_text(f"#!/bin/sh\necho RS-{PAIRED}\n")
    _chmod_x(rust_stub)
    return root


def _run(
    root: Path, tool: str, extra_env: dict[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    env = {
        key: value
        for key, value in os.environ.items()
        if key not in ("DAGRUN_ENGINE", "AGENT_UTILS_ENGINE", "CI_HUB_PYTHON")
    }
    env.update(extra_env or {})
    return subprocess.run(
        [str(root / "common" / "bin" / tool)],
        cwd=root,
        capture_output=True,
        text=True,
        env=env,
        check=False,
        timeout=30,
    )


def test_paired_tool_defaults_to_rust(tmp_path: Path) -> None:
    """With no override, a paired tool runs its tracked Rust launcher."""
    root = _build_sandbox(tmp_path)
    result = _run(root, PAIRED)
    assert result.returncode == 0, result.stderr
    assert result.stdout == f"RS-{PAIRED}\n"
    assert "engine=rust" in result.stderr


def test_python_only_tool_defaults_to_python(tmp_path: Path) -> None:
    """A tool with no Rust launcher runs Python with no override needed."""
    root = _build_sandbox(tmp_path)
    result = _run(root, PYTHON_ONLY)
    assert result.returncode == 0, result.stderr
    assert result.stdout == f"PY-{PYTHON_ONLY}\n"
    assert "engine=python" in result.stderr


def test_explicit_python_still_selects_python(tmp_path: Path) -> None:
    """Both override spellings keep the Python edition reachable."""
    root = _build_sandbox(tmp_path)
    for extra_env in (
        {"DAGRUN_ENGINE": "python"},
        {"DAGRUN_ENGINE": "py"},
        {"AGENT_UTILS_ENGINE": "python"},
    ):
        result = _run(root, PAIRED, extra_env)
        assert result.returncode == 0, result.stderr
        assert result.stdout == f"PY-{PAIRED}\n"


def test_explicit_rust_still_selects_rust(tmp_path: Path) -> None:
    root = _build_sandbox(tmp_path)
    for extra_env in (
        {"DAGRUN_ENGINE": "rust"},
        {"DAGRUN_ENGINE": "rs"},
        {"AGENT_UTILS_ENGINE": "rust"},
    ):
        result = _run(root, PAIRED, extra_env)
        assert result.returncode == 0, result.stderr
        assert result.stdout == f"RS-{PAIRED}\n"


def test_dagrun_engine_wins_when_both_are_set(tmp_path: Path) -> None:
    """The historical variable keeps precedence so existing call sites win."""
    root = _build_sandbox(tmp_path)
    result = _run(
        root,
        PAIRED,
        {"DAGRUN_ENGINE": "python", "AGENT_UTILS_ENGINE": "rust"},
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == f"PY-{PAIRED}\n"


def test_explicit_rust_for_python_only_tool_refuses(tmp_path: Path) -> None:
    """No silent fallback: the Python entrypoint must not run."""
    root = _build_sandbox(tmp_path)
    result = _run(root, PYTHON_ONLY, {"DAGRUN_ENGINE": "rust"})
    assert result.returncode == 127
    assert result.stdout == ""
    assert "Refusing to silently fall back" in result.stderr


def test_invalid_engine_values_refuse(tmp_path: Path) -> None:
    root = _build_sandbox(tmp_path)
    for variable in ("DAGRUN_ENGINE", "AGENT_UTILS_ENGINE"):
        result = _run(root, PAIRED, {variable: "bogus"})
        assert result.returncode == 2
        assert result.stdout == ""
        assert variable in result.stderr
        assert "is invalid" in result.stderr
