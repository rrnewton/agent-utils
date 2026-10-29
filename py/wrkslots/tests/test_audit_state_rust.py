"""Differential: wrkslotsd parse-audit against the Python audit.

#562 rust-primary-wrkslots slice 49. The Python oracle is
`_cmd_audit --format json` on fixture registries with real Git
worktrees; the Rust reader must print the identical payload after
scrubbing live-measured fields (phase metrics, heartbeat ages,
free disk bytes). Temporary fixture registries only.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import io
import json
import os
import subprocess
from pathlib import Path
from typing import cast

from wrkslots import cli as wrkslots_cli


AU_ROOT = Path(__file__).resolve().parents[3]


def _git(cwd: Path, *args: str) -> str:
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(
        {
            "LC_ALL": "C",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_SYSTEM": "/dev/null",
            "GIT_AUTHOR_DATE": "2026-10-26T00:00:00+00:00",
            "GIT_COMMITTER_DATE": "2026-10-26T00:00:00+00:00",
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
    _git(path, "init", "-q", "-b", "main")
    (path / "file.txt").write_text("one\n")
    _git(path, "add", "file.txt")
    _git(path, "commit", "-q", "-m", "one")
    return _git(path, "rev-parse", "HEAD")


def _dead_identity() -> dict[str, object]:
    proc = subprocess.Popen(["sleep", "30"])
    try:
        identity = wrkslots_cli._read_process_identity(proc.pid)
        return {
            "pid": identity.pid,
            "start_ticks": identity.start_ticks,
            "boot_id": identity.boot_id,
            "host_id": identity.host_id,
            "cgroup_path": identity.cgroup_path,
        }
    finally:
        proc.kill()
        proc.wait()


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
    remote = base / "remote.git"
    _git(base, "init", "-q", "--bare", "-b", "main", str(remote))
    _git(upstream, "remote", "add", "origin", str(remote))
    _git(upstream, "push", "-q", "origin", "main")
    _git(
        upstream, "worktree", "add", "-q", "-b", "suite/work",
        str(root / "worktrees" / "slot-a" / "repo"), "HEAD",
    )
    _git(upstream, "push", "-q", "origin", "suite/work")
    identity = _dead_identity()
    record = {
        "slot": "slot-a",
        "agent": "agent-slot-a",
        "task": "do work",
        "purpose": "testing",
        "slot_type": "agent",
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
                "remote_url_sha256": hashlib.sha256(str(remote).encode()).hexdigest(),
                "landed_ref": "refs/remotes/origin/main",
                "head": head,
                "containing_remote_refs": ["refs/remotes/origin/main"],
                "vcs": "git",
            }
        ],
    }
    (root / "worktrees" / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 7, "slots": [record]})
    )
    (root / "worktrees" / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 3, "records": []})
    )
    return root


def _scrub(payload: dict[str, object]) -> dict[str, object]:
    payload = json.loads(json.dumps(payload))
    payload["metrics"] = None
    disk = payload.get("disk")
    if isinstance(disk, dict):
        disk["free_bytes"] = 0
    slots = payload.get("slots")
    if isinstance(slots, list):
        for row in slots:
            if isinstance(row, dict) and "heartbeat_age_seconds" in row:
                row["heartbeat_age_seconds"] = 0
            if isinstance(row, dict) and isinstance(row.get("cache_bytes"), int):
                # Allocated-block counts are filesystem timing
                # sensitive (a directory's first block can appear
                # between the two editions' measurements); the cache
                # test asserts the magnitude separately.
                row["cache_bytes"] = "counted"
    return payload


def _python(root: Path, tmp_path: Path) -> dict[str, object]:
    args = argparse.Namespace(
        project_root=str(root), machine=None, format="json", gate=False,
        cache_census_state=str(tmp_path / "census-state.json"),
        cache_work_limit=100000, cache_wall_seconds=60.0, wait_lock=0.0,
    )
    stream = io.StringIO()
    with contextlib.redirect_stdout(stream):
        assert wrkslots_cli._cmd_audit(args) == 0
    payload: dict[str, object] = json.loads(stream.getvalue())
    return payload


def _rust(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "parse-audit", "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def _rows(payload: dict[str, object]) -> list[dict[str, object]]:
    return cast(list[dict[str, object]], payload["slots"])


def _check(root: Path, tmp_path: Path) -> dict[str, object]:
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    actual = _scrub(json.loads(result.stdout))
    assert actual == _scrub(_python(root, tmp_path))
    return actual


def test_audit_deletable_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    actual = _check(root, tmp_path)
    rows = _rows(actual)
    assert len(rows) == 1
    assert rows[0]["verdict"] == "DELETABLE"
    assert actual["state"] == "actionable"
    assert actual["attention_slots"] == ["slot-a"]


def test_audit_held_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    (root / "worktrees" / "HOLD.6.node-a.slot-a.json").write_text(
        json.dumps(
            {
                "schema": 1,
                "machine": "node-a",
                "slot": "slot-a",
                "held_at": "2026-10-26T02:00:00Z",
                "reason": "owner asked to pause",
            }
        )
    )
    actual = _check(root, tmp_path)
    assert _rows(actual)[0]["verdict"] == "HELD"
    assert _rows(actual)[0]["reasons"] == ["owner asked to pause"]


def test_audit_absent_slot_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    subprocess.run(
        ["rm", "-rf", str(root / "worktrees" / "slot-a")], check=True
    )
    _git(tmp_path / "upstream-repo", "worktree", "prune")
    actual = _check(root, tmp_path)
    assert _rows(actual)[0]["verdict"] == "RECOVERABLE"


def test_audit_unregistered_directory_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    (root / "worktrees" / "slot-b").mkdir()
    actual = _check(root, tmp_path)
    by_slot = {str(row["slot"]): row for row in _rows(actual)}
    assert by_slot["slot-b"]["verdict"] == "BLOCKED"
    assert by_slot["slot-b"]["owner_state"] == "unregistered"
    assert actual["worktree_count"] == 2


def test_audit_cache_bytes_match_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    config = root / ".wrkslots.yml"
    config.write_text(config.read_text() + "cache_globs:\n  - cache-dir\n")
    cache = root / "worktrees" / "slot-a" / "repo" / "cache-dir"
    cache.mkdir(parents=True)
    # Several files: with a single small file the census's own
    # traversal can grow the directory's block allocation between
    # the two editions' measurements on some filesystems.
    for index in range(40):
        (cache / f"f{index}.bin").write_bytes(b"x" * 64)
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    rust_payload = json.loads(result.stdout)
    python_payload = _python(root, tmp_path)
    assert _scrub(rust_payload) == _scrub(python_payload)
    rust_bytes = _rows(rust_payload)[0]["cache_bytes"]
    python_bytes = _rows(python_payload)[0]["cache_bytes"]
    assert isinstance(rust_bytes, int) and isinstance(python_bytes, int)
    assert rust_bytes > 0 and abs(rust_bytes - python_bytes) <= 4096
    row = _rows(rust_payload)[0]
    assert row["verdict"] == "DELETABLE"
    assert row["cache_status"] == "complete"


def _seal_target(slot: str, generation: int) -> dict[str, object]:
    return {
        "slot": slot,
        "generation": generation,
        "path": f"worktrees/validate/{slot}",
        "original_mode": 0o755,
        "identity": [11, 22, 33],
    }


def _seal(
    root: Path,
    targets: list[dict[str, object]],
    *,
    schema: int = 1,
) -> None:
    (root / "worktrees" / "VALIDATE-BATCH-SEAL.node-a.journal").write_text(
        json.dumps(
            {
                "schema": schema,
                "kind": "validate-batch-seal",
                "machine": "node-a",
                "created_at": "2026-10-26T03:00:00Z",
                "actor": _dead_identity(),
                "targets": targets,
            }
        )
    )


def test_audit_seal_matching_generation_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    _seal(root, [_seal_target("slot-a", 1)])
    actual = _check(root, tmp_path)
    row = _rows(actual)[0]
    assert row["verdict"] == "BLOCKED"
    assert row["reasons"] == [
        "interrupted validation-batch seal requires wrkslots recover"
    ]


def test_audit_seal_generation_mismatch_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    _seal(root, [_seal_target("slot-a", 9)])
    actual = _check(root, tmp_path)
    row = _rows(actual)[0]
    assert row["verdict"] == "UNKNOWN"
    assert actual["unknown_slots"] == ["slot-a"]


def test_audit_seal_unrepresented_target_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    _seal(root, [_seal_target("slot-v", 3)])
    actual = _check(root, tmp_path)
    by_slot = {str(row["slot"]): row for row in _rows(actual)}
    assert by_slot["slot-v"]["verdict"] == "BLOCKED"
    assert by_slot["slot-v"]["owner_state"] == "validation-batch-seal"
    assert by_slot["slot-v"]["generation"] == 3


def test_audit_seal_unreadable_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    _seal(root, [_seal_target("slot-a", 1)], schema=2)
    actual = _check(root, tmp_path)
    by_slot = {str(row["slot"]): row for row in _rows(actual)}
    error_row = by_slot["VALIDATE-BATCH-SEAL.node-a.journal"]
    assert error_row["verdict"] == "UNKNOWN"
    assert error_row["reasons"] == [
        "validation-batch seal evidence is unreadable: "
        "unsupported validation-batch seal journal schema"
    ]


def test_audit_seal_unregistered_validate_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    (root / "worktrees" / "validate" / "slot-u").mkdir(parents=True)
    _seal(root, [_seal_target("slot-u", 5)])
    actual = _check(root, tmp_path)
    by_slot = {str(row["slot"]): row for row in _rows(actual)}
    assert by_slot["slot-u"]["owner_state"] == "validation-batch-seal"
    assert by_slot["slot-u"]["generation"] == 5
    assert by_slot["slot-u"]["cache_status"] == "complete"
    assert by_slot["slot-u"]["cache_error"] is None


def test_audit_validate_unregistered_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    (root / "worktrees" / "validate" / "slot-u").mkdir(parents=True)
    actual = _check(root, tmp_path)
    by_slot = {str(row["slot"]): row for row in _rows(actual)}
    assert by_slot["slot-u"]["owner_state"] == "unregistered"
    assert by_slot["slot-u"]["cache_status"] == "complete"
    assert by_slot["slot-u"]["cache_error"] is None


def test_audit_journal_row_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    identity = _dead_identity()
    (root / "worktrees" / "ACTIVE.node-a.journal").write_text(
        json.dumps(
            {
                "schema": 2,
                "kind": "create",
                "machine": "node-a",
                "slot": "slot-j",
                "agent": "agent-slot-j",
                "task": "do work",
                "purpose": "testing",
                "created_at": "2026-10-26T00:00:00Z",
                "heartbeat_at": "2026-10-26T01:00:00Z",
                "heartbeat_ttl_seconds": 60,
                "owner": identity,
                "coordinator_lease": identity,
                "planned": [
                    {
                        "name": "repo",
                        "destination": "worktrees/slot-j/repo",
                        "repository": "../upstream-repo",
                        "branch": "suite/work",
                        "start_point": "a" * 40,
                        "remote": "origin",
                        "remote_url_sha256": "b" * 64,
                        "landed_ref": "refs/remotes/origin/main",
                    }
                ],
                "created": [],
            }
        )
    )
    actual = _check(root, tmp_path)
    by_slot = {str(row["slot"]): row for row in _rows(actual)}
    assert by_slot["slot-j"]["verdict"] == "BLOCKED"
    assert by_slot["slot-j"]["owner_state"] == "create-journal"
