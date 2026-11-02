"""Differential: wrkslotsd classify-create-journals against Python.

#562 rust-primary-wrkslots: the soak `classify-create-journals`
reader. The Python oracle is `_cmd_classify_create_journals` on a
copy of the fixture registry holding one interrupted create (a
failing post-provision hook left its journal); the Rust reader runs
on the original. Stdout must match byte for byte apart from fixture
roots and commit ids. Temporary fixture registries only.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import os
import re
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.rust_binary import rust_wrkslotsd
from wrkslots.tests.test_create_text_rust import _fixture, _repo


AU_ROOT = Path(__file__).resolve().parents[3]
HEX40 = re.compile(r"\b[0-9a-f]{40}\b")


def _norm(text: str, root: Path) -> str:
    text = text.replace(str(root), "ROOT").replace(str(root.parent), "BASE")
    return HEX40.sub("HEAD", text)


def _interrupted(root: Path) -> None:
    config = root / ".wrkslots.yml"
    config.write_text(
        config.read_text() + "post_provision_hooks:\n  - 'echo hook-broken >&2; exit 3'\n"
    )
    args = argparse.Namespace(
        project_root=str(root), machine="node-a", slot="slot-b", slot_type="agent",
        coordinator_authorized=True, agent="agent-new", task="task-1",
        purpose="testing", owner_pid=os.getpid(), coordinator_pid=os.getpid(),
        task_scope_unit=None, task_scope_invocation_id=None,
        repo=["repo=../upstream-repo"], remote=[], remote_url=[],
        branch=["repo=suite/new"], start=[], override_disk_floor=False,
        representation="config", format="human", wait_lock=0.0,
    )
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        with pytest.raises(Exception):
            wrkslots_cli._cmd_create(args)
    assert list((root / "worktrees").glob("CREATE.*.journal"))


def _python_classify(root: Path, *, fmt: str) -> tuple[int, str]:
    args = argparse.Namespace(
        project_root=str(root), machine="node-a", slot="slot-c",
        slot_type="agent", agent="agent-other", format=fmt,
    )
    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer):
        rc = wrkslots_cli._cmd_classify_create_journals(args)
    return rc, buffer.getvalue()


def _rust_classify(root: Path, *, fmt: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            str(rust_wrkslotsd()), "classify-create-journals", "slot-c",
            "--agent", "agent-other", "--slot-type", "agent",
            "--machine", "node-a", "--root", str(root), "--format", fmt,
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
        env=dict(os.environ),
    )


def test_classify_journals_human_matches_python(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    _interrupted(rust_root)
    _interrupted(oracle_root)
    rc, out = _python_classify(oracle_root, fmt="human")
    assert rc == 0
    result = _rust_classify(rust_root, fmt="human")
    assert result.returncode == 0, result.stderr
    assert _norm(result.stdout, rust_root) == _norm(out, oracle_root)
    assert "live-incomplete-create=1" in result.stdout


def test_classify_journals_json_matches_python(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    _interrupted(rust_root)
    _interrupted(oracle_root)
    rc, out = _python_classify(oracle_root, fmt="json")
    assert rc == 0
    result = _rust_classify(rust_root, fmt="json")
    assert result.returncode == 0, result.stderr
    # Journal identity (device/inode/size/sha) differs per fixture file.
    def scrub(text: str) -> str:
        return re.sub(r'"journal_identity": \{[^}]*\}', '"journal_identity": {}', text)
    assert scrub(_norm(result.stdout, rust_root)) == scrub(_norm(out, oracle_root))


def test_classify_journals_empty_matches_python(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    rc, out = _python_classify(oracle_root, fmt="human")
    assert rc == 0
    result = _rust_classify(rust_root, fmt="human")
    assert result.returncode == 0, result.stderr
    assert _norm(result.stdout, rust_root) == _norm(out, oracle_root)
    assert "journals=0" in result.stdout
