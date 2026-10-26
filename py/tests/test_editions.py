"""The editions mirror-state model agrees with the tree and drives the resolver.

`validation/editions.json` (#539 rust-primary-mirror-state) declares each
tool's primary edition and mirror state. `scripts/check_editions.py` proves it
matches the tracked entrypoints and the Rust agentctl python-only refusal
lists; these tests pin the resolver's half: with the file present, the primary
wins by default and a non-parity mirror refuses by name.
"""

from __future__ import annotations

import os
import shutil
import stat
import subprocess
from pathlib import Path


AU_ROOT = Path(__file__).resolve().parents[2]
RESOLVER = AU_ROOT / "common" / "bin" / "engine-resolver"


def _chmod_x(path: Path) -> None:
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def _build_sandbox(tmp_path: Path, *, mirror_state: str) -> Path:
    root = tmp_path / "au"
    (root / "common" / "bin").mkdir(parents=True)
    (root / "py" / "bin").mkdir(parents=True)
    (root / "rs" / "bin").mkdir(parents=True)
    (root / "validation").mkdir(parents=True)
    shutil.copy2(RESOLVER, root / "common" / "bin" / "engine-resolver")
    _chmod_x(root / "common" / "bin" / "engine-resolver")
    (root / "common" / "bin" / "tick-hub").symlink_to("engine-resolver")
    python_stub = root / "py" / "bin" / "tick-hub"
    python_stub.write_text("print('PY-tick-hub')\n")
    _chmod_x(python_stub)
    rust_stub = root / "rs" / "bin" / "tick-hub"
    rust_stub.write_text("#!/bin/sh\necho RS-tick-hub\n")
    _chmod_x(rust_stub)
    paused_at = "abc123" if mirror_state == "paused" else ""
    (root / "validation" / "editions.json").write_text("{}\n")
    (root / "validation" / "editions.tsv").write_text(
        f"tick-hub\trust\t{mirror_state}\t{paused_at}\n"
    )
    return root


def _run(root: Path, extra_env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    env = {
        key: value
        for key, value in os.environ.items()
        if key not in ("DAGRUN_ENGINE", "AGENT_UTILS_ENGINE", "CI_HUB_PYTHON")
    }
    env.update(extra_env or {})
    return subprocess.run(
        [str(root / "common" / "bin" / "tick-hub")],
        cwd=root,
        capture_output=True,
        text=True,
        env=env,
        check=False,
        timeout=30,
    )


def test_checker_agrees_with_the_real_tree() -> None:
    result = subprocess.run(
        ["python3", "scripts/check_editions.py"],
        cwd=AU_ROOT,
        capture_output=True,
        text=True,
        check=False,
        timeout=60,
    )
    assert result.returncode == 0, result.stderr
    assert "check-editions: OK" in result.stdout


def test_editions_primary_wins_by_default(tmp_path: Path) -> None:
    root = _build_sandbox(tmp_path, mirror_state="parity")
    result = _run(root)
    assert result.returncode == 0, result.stderr
    assert result.stdout == "RS-tick-hub\n"


def test_parity_mirror_stays_reachable(tmp_path: Path) -> None:
    root = _build_sandbox(tmp_path, mirror_state="parity")
    result = _run(root, {"DAGRUN_ENGINE": "python"})
    assert result.returncode == 0, result.stderr
    assert result.stdout == "PY-tick-hub\n"


def test_paused_mirror_refuses_and_names_its_state(tmp_path: Path) -> None:
    root = _build_sandbox(tmp_path, mirror_state="paused")
    result = _run(root, {"DAGRUN_ENGINE": "python"})
    assert result.returncode == 127
    assert result.stdout == ""
    assert "paused at abc123" in result.stderr
    assert "Refusing to silently fall back" in result.stderr


def test_absent_mirror_refuses(tmp_path: Path) -> None:
    root = _build_sandbox(tmp_path, mirror_state="absent")
    result = _run(root, {"DAGRUN_ENGINE": "python"})
    assert result.returncode == 127
    assert "absent mirror" in result.stderr
