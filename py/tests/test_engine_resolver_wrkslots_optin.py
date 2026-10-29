"""WRKSLOTS_ENGINE opt-in for the wrkslots Rust edition trial.

#562 rust-primary-wrkslots: `wrkslots` stays Python-primary by
default. `WRKSLOTS_ENGINE=rust` is the per-tool opt-in that
dispatches to the Rust edition (the wrkslotsd binary through its
tracked launcher); it wins over the generic engine overrides for
this tool only, and the generic overrides keep refusing a Rust
request for wrkslots, whose mirror is not at parity. The sandbox
uses stub launchers, so these tests pin the dispatch decision
without building any Rust.
"""

from __future__ import annotations

import os
import shutil
import stat
import subprocess
from pathlib import Path


AU_ROOT = Path(__file__).resolve().parents[2]
RESOLVER = AU_ROOT / "common" / "bin" / "engine-resolver"
TOOL = "wrkslots"


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
    (common_bin / TOOL).symlink_to("engine-resolver")
    stub = python_bin / TOOL
    stub.write_text(f"print('PY-{TOOL}')\n")
    _chmod_x(stub)
    rust_stub = rust_bin / "wrkslotsd"
    rust_stub.write_text("#!/bin/sh\necho RS-wrkslotsd\n")
    _chmod_x(rust_stub)
    return root


def _run(
    root: Path, extra_env: dict[str, str] | None = None
) -> subprocess.CompletedProcess[str]:
    env = {
        key: value
        for key, value in os.environ.items()
        if key
        not in ("DAGRUN_ENGINE", "AGENT_UTILS_ENGINE", "WRKSLOTS_ENGINE", "CI_HUB_PYTHON")
    }
    env.update(extra_env or {})
    return subprocess.run(
        [str(root / "common" / "bin" / TOOL)],
        cwd=root,
        capture_output=True,
        text=True,
        env=env,
        check=False,
        timeout=30,
    )


def test_wrkslots_defaults_to_python(tmp_path: Path) -> None:
    root = _build_sandbox(tmp_path)
    result = _run(root)
    assert result.returncode == 0, result.stderr
    assert result.stdout == f"PY-{TOOL}\n"
    assert "engine=python" in result.stderr


def test_wrkslots_engine_rust_selects_rust_edition(tmp_path: Path) -> None:
    root = _build_sandbox(tmp_path)
    for value in ("rust", "rs"):
        result = _run(root, {"WRKSLOTS_ENGINE": value})
        assert result.returncode == 0, result.stderr
        assert result.stdout == "RS-wrkslotsd\n"
        assert "engine=rust" in result.stderr


def test_wrkslots_engine_python_keeps_python(tmp_path: Path) -> None:
    root = _build_sandbox(tmp_path)
    result = _run(root, {"WRKSLOTS_ENGINE": "python"})
    assert result.returncode == 0, result.stderr
    assert result.stdout == f"PY-{TOOL}\n"


def test_wrkslots_engine_wins_over_generic_overrides(tmp_path: Path) -> None:
    root = _build_sandbox(tmp_path)
    result = _run(
        root, {"WRKSLOTS_ENGINE": "rust", "DAGRUN_ENGINE": "python"}
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == "RS-wrkslotsd\n"
    result = _run(
        root, {"WRKSLOTS_ENGINE": "python", "DAGRUN_ENGINE": "rust"}
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == f"PY-{TOOL}\n"


def test_generic_rust_request_for_wrkslots_still_refuses(tmp_path: Path) -> None:
    root = _build_sandbox(tmp_path)
    result = _run(root, {"DAGRUN_ENGINE": "rust"})
    assert result.returncode == 127
    assert result.stdout == ""
    assert "Refusing to silently fall back" in result.stderr


def test_wrkslots_engine_invalid_value_refuses(tmp_path: Path) -> None:
    root = _build_sandbox(tmp_path)
    result = _run(root, {"WRKSLOTS_ENGINE": "bogus"})
    assert result.returncode == 2
    assert result.stdout == ""
    assert "WRKSLOTS_ENGINE" in result.stderr
