"""Differential: wrkslotsd sandbox-show-config against Python.

#1101/#1116 image glue batch. The Python oracle is
`imagecmd._cmd_sandbox_show` in-process; the Rust side runs the
built wrkslotsd binary with the Python spelling. Env keys are
deliberately out of alphabetical order in the custom case: Python
renders them in document order, and the Rust glue must too.
"""

from __future__ import annotations

import argparse
import os
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli
from wrkslots import imagecmd

from wrkslots.tests.rust_binary import rust_wrkslotsd


def _root(tmp_path: Path, sandbox: str = "") -> Path:
    root = tmp_path / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: node-a\n"
        "worktrees_dir: worktrees\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
        + sandbox
    )
    return root


def _python(root: Path, capsys: pytest.CaptureFixture[str]) -> tuple[int, str]:
    args = argparse.Namespace(project_root=str(root), machine="node-a")
    rc = imagecmd._cmd_sandbox_show(args)
    return rc, capsys.readouterr().out


def _rust(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(rust_wrkslotsd()), "sandbox", "show-config"],
        cwd=root, capture_output=True, text=True, check=False, timeout=900,
        env={**os.environ, "WRKSLOTS_PYCOMPAT": "1"},
    )


def test_sandbox_show_config_defaults_match_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path)
    expected = _python(root, capsys)
    result = _rust(root)
    assert (result.returncode, result.stdout) == expected
    assert result.stderr == ""


def test_sandbox_show_config_custom_env_order_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(
        tmp_path,
        "sandbox:\n"
        "  isolation: root\n"
        "  env:\n"
        "    ZED: '1'\n"
        "    ALPHA: ~/x\n"
        "  limits:\n"
        "    memory_max: 32G\n"
        "    cpu_quota: 800%\n"
        "  home_expose:\n"
        "    - bin\n",
    )
    expected = _python(root, capsys)
    result = _rust(root)
    assert (result.returncode, result.stdout) == expected
    assert result.stderr == ""
    assert expected[1].index('"ZED"') < expected[1].index('"ALPHA"')


def test_sandbox_show_config_invalid_refused_by_both(tmp_path: Path) -> None:
    root = _root(tmp_path, "sandbox:\n  isolation: namespace\n")
    args = argparse.Namespace(project_root=str(root), machine="node-a")
    with pytest.raises(wrkslots_cli.Refusal) as caught:
        imagecmd._cmd_sandbox_show(args)
    result = _rust(root)
    assert result.returncode != 0
    assert str(caught.value) in result.stderr


def test_sandbox_write_defaults_stays_refused(tmp_path: Path) -> None:
    root = _root(tmp_path)
    result = subprocess.run(
        [str(rust_wrkslotsd()), "sandbox", "write-defaults"],
        cwd=root, capture_output=True, text=True, check=False, timeout=900,
        env={**os.environ, "WRKSLOTS_PYCOMPAT": "1"},
    )
    assert result.returncode == 1
    assert "'sandbox' stays with the Python edition in this slice" in result.stderr
