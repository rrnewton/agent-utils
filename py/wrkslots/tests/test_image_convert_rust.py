"""Differential: wrkslotsd image-convert registry checks.

#1291 migrate/convert. The conversion itself needs slot-image
provisioning (Python-only in this slice), so the wired Rust side
runs the registry checks and the already-target outcome, then
refuses the conversion by name; those checks must match Python
byte for byte.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli
from wrkslots import imagecmd

from wrkslots.tests.rust_binary import rust_wrkslotsd


def _root(tmp_path: Path) -> Path:
    upstream = tmp_path / "upstream-repo"
    upstream.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["git", "init", "-q", str(upstream)], check=False, capture_output=True
    )
    root = tmp_path / "project"
    control = root / "worktrees"
    (control / "slot-a").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: node-a\n"
        "worktrees_dir: worktrees\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
    )
    identity = wrkslots_cli._read_process_identity(os.getpid())
    lease = {
        "pid": os.getpid(),
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    }
    record = {
        "slot": "slot-a",
        "agent": "agent-a",
        "task": "task-1",
        "purpose": "testing",
        "slot_type": "agent",
        "machine": "node-a",
        "generation": 1,
        "created_at": "2026-11-07T00:00:00+00:00",
        "heartbeat_at": "2026-11-07T00:00:00+00:00",
        "heartbeat_ttl_seconds": 60,
        "owner": lease,
        "coordinator_lease": lease,
        "coordinator_recovery_note": None,
        "handoff": None,
        "checkouts": [
            {
                "name": "repo",
                "path": "worktrees/slot-a/repo",
                "repository": "../upstream-repo",
                "branch": "suite/work",
                "start_point": "a" * 40,
                "remote": "origin",
                "remote_url_sha256": "b" * 64,
                "landed_ref": "refs/remotes/origin/main",
                "head": "a" * 40,
                "containing_remote_refs": ["refs/remotes/origin/main"],
                "vcs": "git",
            }
        ],
    }
    (control / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 1, "slots": [record]})
    )
    (control / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 0, "records": []})
    )
    return root


def _python(
    root: Path,
    capsys: pytest.CaptureFixture[str],
    slot: str,
    slot_type: str,
    target: str,
) -> tuple[int, str, str]:
    args = argparse.Namespace(
        project_root=str(root), machine="node-a", wait_lock=0,
        slot=slot, slot_type=slot_type, target=target, keep_original=False,
    )
    try:
        rc = imagecmd._cmd_image_convert(args)
        err = ""
    except wrkslots_cli.Refusal as exc:
        rc = 3
        remedy = wrkslots_cli._refusal_remedy(argparse.Namespace(command="image"), exc)
        err = f"REFUSED: {exc}\nREMEDY: {remedy}\n"
    captured = capsys.readouterr()
    return rc, captured.out, err or captured.err


def _rust(root: Path, slot: str, slot_type: str, target: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            str(rust_wrkslotsd()), "image", "convert", slot,
            "--slot-type", slot_type, "--to", target,
        ],
        cwd=root, capture_output=True, text=True, check=False, timeout=900,
        env={
            **os.environ,
            "WRKSLOTS_PYCOMPAT": "1",
            "WRKSLOTSD_FIXTURE_WRITER": "1",
        },
    )


def test_convert_already_worktree_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path)
    expected = _python(root, capsys, "slot-a", "agent", "worktree")
    result = _rust(root, "slot-a", "agent", "worktree")
    assert (result.returncode, result.stdout, result.stderr) == expected


def test_convert_unknown_slot_refusal_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path)
    expected = _python(root, capsys, "slot-z", "agent", "image")
    result = _rust(root, "slot-z", "agent", "image")
    assert (result.returncode, result.stdout, result.stderr) == expected


def test_convert_type_mismatch_refusal_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path)
    expected = _python(root, capsys, "slot-a", "validate", "image")
    result = _rust(root, "slot-a", "validate", "image")
    assert (result.returncode, result.stdout, result.stderr) == expected


def test_convert_to_image_refuses_provisioning_by_name(tmp_path: Path) -> None:
    root = _root(tmp_path)
    result = _rust(root, "slot-a", "agent", "image")
    assert result.returncode == 3
    assert "image conversion stays with the Python edition" in result.stderr
