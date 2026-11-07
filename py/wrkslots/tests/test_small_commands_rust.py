"""Differential: wrkslotsd small commands against the Python oracle.

#562 rust-primary-wrkslots P3/P4 slice: `quickstart`, `retirement-queue`,
`register`, `supersede-handoff`, and `config convert`. Each case runs the
Python handler on a copy of the fixture registry and the Rust command on
the original; stdout and the resulting state must match apart from
wall-clock stamps, process identities, and fixture-specific paths.
Temporary fixture registries only: writers are never pointed at a live
registry before the edition cutover.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import os
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.rust_binary import rust_wrkslotsd


AU_ROOT = Path(__file__).resolve().parents[3]
DIGEST = "b" * 64


def _git(cwd: Path, *args: str) -> str:
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(
        {
            "LC_ALL": "C",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_SYSTEM": "/dev/null",
            "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z",
            "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z",
        }
    )
    result = subprocess.run(
        [
            "git", "-c", "user.email=test@example.invalid", "-c", "user.name=Test",
            "-c", "commit.gpgsign=false", "-C", str(cwd), *args,
        ],
        capture_output=True, text=True, check=True, env=env, timeout=120,
    )
    return result.stdout.strip()


def _repo(path: Path) -> str:
    path.mkdir(parents=True)
    _git(path, "init", "-q")
    (path / "file.txt").write_text("one\n")
    _git(path, "add", "file.txt")
    _git(path, "commit", "-q", "-m", "one")
    return _git(path, "rev-parse", "HEAD")


def _live_identity() -> dict[str, object]:
    identity = wrkslots_cli._read_process_identity(os.getpid())
    return {
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    }


def _record(head: str) -> dict[str, object]:
    identity = _live_identity()
    return {
        "slot": "slot-a",
        "agent": "agent-slot-a",
        "task": "do work",
        "purpose": "testing",
        "machine": "node-a",
        "generation": 1,
        "created_at": "2026-10-26T00:00:00Z",
        "heartbeat_at": "2026-10-26T01:00:00Z",
        "heartbeat_ttl_seconds": 60,
        "owner": identity,
        "coordinator_lease": identity,
        "coordinator_recovery_note": None,
        "handoff": None,
        "checkouts": [
            {
                "name": "repo",
                "path": "worktrees/slot-a/repo",
                "repository": "../upstream-repo",
                "branch": "suite/work",
                "start_point": head,
                "remote": "origin",
                "remote_url_sha256": DIGEST,
                "landed_ref": "refs/remotes/origin/main",
                "head": head,
                "containing_remote_refs": ["refs/remotes/origin/main"],
                "vcs": "git",
            }
        ],
    }


def _fixture(base: Path) -> Path:
    root = base / "project"
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
    )
    upstream = base / "upstream-repo"
    head = _repo(upstream)
    _git(upstream, "remote", "add", "origin", str(upstream))
    _git(
        upstream, "worktree", "add", "-q", "-b", "suite/work",
        str(root / "worktrees" / "slot-a" / "repo"), "HEAD",
    )
    (root / "worktrees" / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 7, "slots": [_record(head)]})
    )
    (root / "worktrees" / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 3, "records": []})
    )
    return root


def _pair(tmp_path: Path) -> tuple[Path, Path]:
    return _fixture(tmp_path / "rust"), _fixture(tmp_path / "oracle")


def _run(handler: object, args: argparse.Namespace) -> tuple[int, str]:
    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer):
        code = handler(args)  # type: ignore[operator]
    return int(code), buffer.getvalue()


def _rust(root: Path, *argv: str, fixture: bool = True) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    return subprocess.run(
        [str(rust_wrkslotsd()), *argv, "--root", str(root), "--machine", "node-a"],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _handoff_setup(root: Path, source: Path) -> None:
    source.write_text("# Handoff\n\nAll done.\n")
    code, _ = _run(
        wrkslots_cli._cmd_write_handoff,
        argparse.Namespace(
            project_root=str(root), machine=None, slot="slot-a", agent="agent-slot-a",
            owner_pid=os.getpid(), expected_generation=1, from_file=str(source),
            wait_lock=0.0,
        ),
    )
    assert code == 0
    code, _ = _run(
        wrkslots_cli._cmd_read_handoff,
        argparse.Namespace(
            project_root=str(root), machine=None, slot="slot-a",
            coordinator_pid=os.getpid(), coordinator_authorized=True,
            adopt_legacy_change=False, expected_generation=None, wait_lock=0.0,
        ),
    )
    assert code == 0


def test_quickstart_matches_python() -> None:
    _, expected = _run(wrkslots_cli._cmd_quickstart, argparse.Namespace())
    result = subprocess.run(
        [str(rust_wrkslotsd()), "quickstart"],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
        env=dict(os.environ),
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == expected


def test_config_convert_yaml_to_json_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    code, expected = _run(
        wrkslots_cli._cmd_config_convert,
        argparse.Namespace(
            project_root=str(oracle_root), machine="node-a", target="json", wait_lock=0.0
        ),
    )
    assert code == 0
    result = _rust(rust_root, "config", "convert", "--to", "json")
    assert result.returncode == 0, result.stderr
    assert result.stdout.replace(str(rust_root), "ROOT") == expected.replace(
        str(oracle_root), "ROOT"
    )
    assert (rust_root / ".wrkslots.yml").read_bytes() == (
        oracle_root / ".wrkslots.yml"
    ).read_bytes()
    assert (rust_root / ".wrkslots.yml.bak").read_bytes() == (
        oracle_root / ".wrkslots.yml.bak"
    ).read_bytes()


def test_config_convert_json_to_yaml_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    payload = {
        "schema": 2,
        "machine": "node-a",
        "worktrees_dir": "worktrees",
        "default_remote": "origin",
        "default_landed_ref": "refs/remotes/origin/main",
        "heartbeat_ttl_seconds": 60,
        "liveness_command": "live.py",
    }
    for root in (rust_root, oracle_root):
        (root / ".wrkslots.yml").write_text(json.dumps(payload, indent=2) + "\n")
    code, expected = _run(
        wrkslots_cli._cmd_config_convert,
        argparse.Namespace(
            project_root=str(oracle_root), machine="node-a", target="yaml", wait_lock=0.0
        ),
    )
    assert code == 0
    result = _rust(rust_root, "config", "convert", "--to", "yaml")
    assert result.returncode == 0, result.stderr
    assert result.stdout.replace(str(rust_root), "ROOT") == expected.replace(
        str(oracle_root), "ROOT"
    )
    assert (rust_root / ".wrkslots.yml").read_bytes() == (
        oracle_root / ".wrkslots.yml"
    ).read_bytes()
    assert (rust_root / ".wrkslots.yml.bak").read_bytes() == (
        oracle_root / ".wrkslots.yml.bak"
    ).read_bytes()


def test_config_convert_refuses_without_fixture_attestation(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "plain")
    before = (rust_root / ".wrkslots.yml").read_bytes()
    result = _rust(rust_root, "config", "convert", "--to", "json", fixture=False)
    assert result.returncode == 1
    assert "fixture-only" in result.stderr
    assert (rust_root / ".wrkslots.yml").read_bytes() == before


def test_retirement_queue_empty_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    for fmt in ("human", "json"):
        _, expected = _run(
            wrkslots_cli._cmd_retirement_queue,
            argparse.Namespace(
                project_root=str(oracle_root), machine=None, format=fmt, wait_lock=0.0
            ),
        )
        result = _rust(rust_root, "retirement-queue", "--format", fmt)
        assert result.returncode == 0, result.stderr
        assert result.stdout == expected


def test_retirement_queue_pending_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    _handoff_setup(rust_root, tmp_path / "rust-notes.md")
    _handoff_setup(oracle_root, tmp_path / "oracle-notes.md")
    _, expected = _run(
        wrkslots_cli._cmd_retirement_queue,
        argparse.Namespace(
            project_root=str(oracle_root), machine=None, format="human", wait_lock=0.0
        ),
    )
    result = _rust(rust_root, "retirement-queue", "--format", "human")
    assert result.returncode == 0, result.stderr
    assert result.stdout == expected
    assert result.stdout.startswith("pending=1\nPENDING: node-a/slot-a generation=1 sha256=")
    _, expected_json = _run(
        wrkslots_cli._cmd_retirement_queue,
        argparse.Namespace(
            project_root=str(oracle_root), machine=None, format="json", wait_lock=0.0
        ),
    )
    result = _rust(rust_root, "retirement-queue", "--format", "json")
    assert result.returncode == 0, result.stderr
    rust_payload = json.loads(result.stdout)
    oracle_payload = json.loads(expected_json)
    for payload, root in ((rust_payload, rust_root), (oracle_payload, oracle_root)):
        for row in payload["pending"]:
            row["enqueued_at"] = "T"
            if row.get("artifact_path"):
                row["artifact_path"] = row["artifact_path"].replace(str(root), "ROOT")
    assert rust_payload == oracle_payload


def _active_normalized(root: Path) -> dict[str, object]:
    doc: dict[str, object] = json.loads((root / "worktrees" / "ACTIVE.node-a.json").read_text())
    for record in doc["slots"]:  # type: ignore[attr-defined]
        record.pop("created_at", None)
        record.pop("heartbeat_at", None)
        record["owner"] = "OWNER"
        record["coordinator_lease"] = "COORDINATOR"
        for checkout in record.get("checkouts", []):
            checkout["head"] = "HEAD"
            checkout["start_point"] = "HEAD"
            checkout["remote_url_sha256"] = "SHA"
    return doc


def test_supersede_handoff_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    _handoff_setup(rust_root, tmp_path / "rust-notes.md")
    _handoff_setup(oracle_root, tmp_path / "oracle-notes.md")
    outputs: dict[str, tuple[int, str]] = {}
    for tag, root in (("rust", rust_root), ("oracle", oracle_root)):
        sleeper = subprocess.Popen(
            ["sleep", "60"], cwd=root / "worktrees" / "slot-a"
        )
        try:
            if tag == "rust":
                result = _rust(
                    root, "supersede-handoff", "slot-a", "--agent", "agent-two",
                    "--owner-pid", str(sleeper.pid), "--expected-generation", "1",
                    "--coordinator-pid", str(os.getpid()), "--coordinator-authorized",
                    "--reason", "lane continues",
                )
                outputs[tag] = (
                    result.returncode,
                    result.stdout.replace(str(sleeper.pid), "PID"),
                )
                assert result.returncode == 0, result.stderr
            else:
                code, out = _run(
                    wrkslots_cli._cmd_supersede_handoff,
                    argparse.Namespace(
                        project_root=str(root), machine=None, slot="slot-a",
                        agent="agent-two", owner_pid=sleeper.pid, expected_generation=1,
                        coordinator_pid=os.getpid(), coordinator_authorized=True,
                        reason="lane continues", wait_lock=0.0,
                    ),
                )
                outputs[tag] = (code, out.replace(str(sleeper.pid), "PID"))
        finally:
            sleeper.terminate()
    assert outputs["rust"] == outputs["oracle"]
    assert _active_normalized(rust_root) == _active_normalized(oracle_root)


def test_supersede_handoff_without_candidate_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    with pytest.raises(Exception) as caught:
        _run(
            wrkslots_cli._cmd_supersede_handoff,
            argparse.Namespace(
                project_root=str(oracle_root), machine=None, slot="slot-a",
                agent="agent-two", owner_pid=os.getpid(), expected_generation=1,
                coordinator_pid=os.getpid(), coordinator_authorized=True,
                reason="lane continues", wait_lock=0.0,
            ),
        )
    assert "not a handoff-read retirement candidate" in str(caught.value)
    result = _rust(
        rust_root, "supersede-handoff", "slot-a", "--agent", "agent-two",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        "--coordinator-pid", str(os.getpid()), "--coordinator-authorized",
        "--reason", "lane continues",
    )
    assert result.returncode == 1
    assert "not a handoff-read retirement candidate" in result.stderr


def test_register_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    for root, branch in ((rust_root, "suite/work-b"), (oracle_root, "suite/work-b")):
        upstream = root.parent / "upstream-repo"
        _git(
            upstream, "worktree", "add", "-q", "-b", branch,
            str(root / "worktrees" / "slot-b" / "repo"), "HEAD",
        )
    code, expected = _run(
        wrkslots_cli._cmd_register,
        argparse.Namespace(
            project_root=str(oracle_root), machine="node-a", slot="slot-b",
            slot_type="agent", coordinator_authorized=True, agent="agent-slot-b",
            task="do work", purpose="testing", owner_pid=os.getpid(),
            coordinator_pid=os.getpid(), verified_live=True,
            repo=["repo=../upstream-repo"], remote=[], remote_url=[],
            task_scope_unit=None, task_scope_invocation_id=None, wait_lock=0.0,
        ),
    )
    assert code == 0
    result = _rust(
        rust_root, "register", "slot-b", "--slot-type", "agent",
        "--coordinator-authorized", "--agent", "agent-slot-b", "--task", "do work",
        "--purpose", "testing", "--owner-pid", str(os.getpid()),
        "--coordinator-pid", str(os.getpid()), "--verified-live",
        "--repo", "repo=../upstream-repo",
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == expected
    assert result.stdout == (
        "registered slot=slot-b agent=agent-slot-b generation=1 checkouts=1\n"
    )
    assert _active_normalized(rust_root) == _active_normalized(oracle_root)


def test_register_already_active_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    with pytest.raises(Exception) as caught:
        _run(
            wrkslots_cli._cmd_register,
            argparse.Namespace(
                project_root=str(oracle_root), machine="node-a", slot="slot-a",
                slot_type="agent", coordinator_authorized=True, agent="agent-other",
                task="do work", purpose="testing", owner_pid=os.getpid(),
                coordinator_pid=os.getpid(), verified_live=True,
                repo=["repo=../upstream-repo"], remote=[], remote_url=[],
                task_scope_unit=None, task_scope_invocation_id=None, wait_lock=0.0,
            ),
        )
    assert "already active" in str(caught.value)
    result = _rust(
        rust_root, "register", "slot-a", "--slot-type", "agent",
        "--coordinator-authorized", "--agent", "agent-other", "--task", "do work",
        "--purpose", "testing", "--owner-pid", str(os.getpid()),
        "--coordinator-pid", str(os.getpid()), "--verified-live",
        "--repo", "repo=../upstream-repo",
    )
    assert result.returncode == 1
    assert "already active" in result.stderr
