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

import pytest

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


def _python(
    root: Path,
    tmp_path: Path,
    *,
    work_limit: int = 100000,
    state_name: str = "census-state.json",
) -> dict[str, object]:
    args = argparse.Namespace(
        project_root=str(root), machine=None, format="json", gate=False,
        cache_census_state=str(tmp_path / state_name),
        cache_work_limit=work_limit, cache_wall_seconds=60.0, wait_lock=0.0,
    )
    stream = io.StringIO()
    with contextlib.redirect_stdout(stream):
        assert wrkslots_cli._cmd_audit(args) == 0
    payload: dict[str, object] = json.loads(stream.getvalue())
    return payload


def _rust(
    root: Path,
    *,
    census_state: Path | None = None,
    work_limit: int | None = None,
) -> subprocess.CompletedProcess[str]:
    command = ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
               "-p", "wrkslotsd", "--", "parse-audit", "--root", str(root)]
    if census_state is not None:
        command += ["--cache-census-state", str(census_state)]
    if work_limit is not None:
        command += ["--cache-work-limit", str(work_limit), "--cache-wall-seconds", "60"]
    return subprocess.run(
        command,
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def _rows(payload: dict[str, object]) -> list[dict[str, object]]:
    return cast(list[dict[str, object]], payload["slots"])


def _check(root: Path, tmp_path: Path) -> dict[str, object]:
    result = _rust(root, census_state=tmp_path / "rust-census.json")
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


def _ghost_checkout(root: Path, base: Path) -> Path:
    checkout = root / "worktrees" / "ghost" / "repo"
    _git(
        base / "upstream-repo", "worktree", "add", "-q", "-b", "suite/ghost",
        str(checkout), "HEAD",
    )
    return checkout


def test_audit_unregistered_cache_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    config = root / ".wrkslots.yml"
    config.write_text(config.read_text() + "cache_globs:\n  - cache\n")
    checkout = _ghost_checkout(root, tmp_path)
    cache = checkout / "cache"
    cache.mkdir(parents=True)
    for index in range(10):
        (cache / f"f{index}.bin").write_bytes(b"x" * 1024)
    actual = _check(root, tmp_path)
    row = {str(item["slot"]): item for item in _rows(actual)}["ghost"]
    assert row["cache_status"] == "complete"
    assert row["cache_error"] is None
    # Scrubbed sentinel: an integer byte count was published.
    assert row["cache_bytes"] == "counted"


def test_audit_unregistered_non_checkout_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    ghost = root / "worktrees" / "ghost"
    _repo(ghost)
    (ghost / "file.txt").unlink()
    (ghost / "inner").mkdir()
    actual = _check(root, tmp_path)
    row = {str(item["slot"]): item for item in _rows(actual)}["ghost"]
    assert row["cache_status"] == "error"
    # The first entry judged (the repository's own .git directory)
    # is refused by Git in both editions, with the same spelling.
    assert "Git refused in" in str(row["cache_error"])


def test_audit_unregistered_file_entry_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    ghost = root / "worktrees" / "ghost"
    ghost.mkdir(parents=True)
    (ghost / "notes.txt").write_text("hi\n")
    actual = _check(root, tmp_path)
    row = {str(item["slot"]): item for item in _rows(actual)}["ghost"]
    assert row["cache_status"] == "error"
    assert "contains a non-checkout entry" in str(row["cache_error"])


def test_audit_unregistered_repo_globs_only_matches_python(
    tmp_path: Path,
) -> None:
    root = _fixture(tmp_path)
    config = root / ".wrkslots.yml"
    config.write_text(
        config.read_text() + "repo_cache_globs:\n  repo:\n    - cache\n"
    )
    _ghost_checkout(root, tmp_path)
    actual = _check(root, tmp_path)
    row = {str(item["slot"]): item for item in _rows(actual)}["ghost"]
    assert row["cache_status"] == "error"
    assert "no trusted repository-name mapping" in str(row["cache_error"])


def test_audit_unregistered_tracked_cache_matches_python(
    tmp_path: Path,
) -> None:
    root = _fixture(tmp_path)
    config = root / ".wrkslots.yml"
    config.write_text(config.read_text() + "cache_globs:\n  - cache\n")
    checkout = _ghost_checkout(root, tmp_path)
    tracked = checkout / "cache" / "tracked.bin"
    tracked.parent.mkdir(parents=True)
    tracked.write_bytes(b"x" * 16)
    _git(checkout, "add", "cache/tracked.bin")
    _git(checkout, "commit", "-q", "-m", "tracked cache")
    actual = _check(root, tmp_path)
    row = {str(item["slot"]): item for item in _rows(actual)}["ghost"]
    assert row["cache_status"] == "error"
    assert "overlaps tracked source at cache/tracked.bin" in str(
        row["cache_error"]
    )


def test_audit_unregistered_symlink_ancestor_matches_python(
    tmp_path: Path,
) -> None:
    root = _fixture(tmp_path)
    config = root / ".wrkslots.yml"
    config.write_text(config.read_text() + "cache_globs:\n  - link/cache\n")
    checkout = _ghost_checkout(root, tmp_path)
    real = checkout / "real"
    (real / "cache").mkdir(parents=True)
    (real / "cache" / "f.bin").write_bytes(b"x" * 1024)
    (checkout / "link").symlink_to(real, target_is_directory=True)
    actual = _check(root, tmp_path)
    row = {str(item["slot"]): item for item in _rows(actual)}["ghost"]
    assert row["cache_status"] == "error"
    assert "cache path crosses a symlink" in str(row["cache_error"])


def test_audit_symlink_file_candidate_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    config = root / ".wrkslots.yml"
    config.write_text(config.read_text() + "cache_globs:\n  - cache\n")
    checkout = root / "worktrees" / "slot-a" / "repo"
    (checkout / "target-file").write_bytes(b"x" * 16)
    (checkout / "cache").symlink_to(checkout / "target-file")
    actual = _check(root, tmp_path)
    row = {str(item["slot"]): item for item in _rows(actual)}["slot-a"]
    assert row["cache_status"] == "error"
    assert "cache path crosses a symlink" in str(row["cache_error"])


def test_audit_planning_work_exhaustion_matches_python(
    tmp_path: Path,
) -> None:
    root = _cache_fixture(tmp_path)
    config = root / ".wrkslots.yml"
    config.write_text(config.read_text().replace("- cache\n", "- cache/**\n"))
    python_payload = _python(root, tmp_path, work_limit=5)
    rust_result = _rust(
        root, census_state=tmp_path / "rust-census.json", work_limit=5
    )
    assert rust_result.returncode == 0, rust_result.stderr
    assert _scrub(json.loads(rust_result.stdout)) == _scrub(python_payload)
    row = _rows(python_payload)[0]
    assert row["cache_status"] == "error"
    assert "share of 5 directory listings" in str(row["cache_error"])


def test_audit_planning_fair_share_matches_python(tmp_path: Path) -> None:
    root = _cache_fixture(tmp_path)
    config = root / ".wrkslots.yml"
    config.write_text(config.read_text().replace("- cache\n", "- cache/**\n"))
    checkout = _ghost_checkout(root, tmp_path)
    cache = checkout / "cache"
    for index in range(30):
        directory = cache / f"d{index}"
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "f.bin").write_bytes(b"x" * 1024)
    python_payload = _python(root, tmp_path, work_limit=6)
    rust_result = _rust(
        root, census_state=tmp_path / "rust-census.json", work_limit=6
    )
    assert rust_result.returncode == 0, rust_result.stderr
    assert _scrub(json.loads(rust_result.stdout)) == _scrub(python_payload)
    by_slot = {str(item["slot"]): item for item in _rows(python_payload)}
    # Two planning targets: each is offered half of the six
    # listings, and each exhausts exactly that share.
    for slot in ("slot-a", "ghost"):
        assert by_slot[slot]["cache_status"] == "error"
        assert "share of 3 directory listings" in str(
            by_slot[slot]["cache_error"]
        )


def test_audit_wrong_repository_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    _repo(tmp_path / "other-repo")
    active_path = root / "worktrees" / "ACTIVE.node-a.json"
    active = json.loads(active_path.read_text())
    active["slots"][0]["checkouts"][0]["repository"] = "../other-repo"
    active_path.write_text(json.dumps(active))
    config = root / ".wrkslots.yml"
    config.write_text(config.read_text() + "cache_globs:\n  - cache\n")
    cache = root / "worktrees" / "slot-a" / "repo" / "cache"
    cache.mkdir(parents=True)
    (cache / "f.bin").write_bytes(b"x" * 1024)
    actual = _check(root, tmp_path)
    row = _rows(actual)[0]
    assert row["cache_status"] == "error"
    assert "does not belong to repository" in str(row["cache_error"])


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
    result = _rust(root, census_state=tmp_path / "rust-census.json")
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


def _cache_fixture(base: Path) -> Path:
    root = _fixture(base)
    config = root / ".wrkslots.yml"
    config.write_text(config.read_text() + "cache_globs:\n  - cache\n")
    cache = root / "worktrees" / "slot-a" / "repo" / "cache"
    for index in range(30):
        directory = cache / f"d{index}"
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "f.bin").write_bytes(b"x" * 1024)
    return root


def test_audit_census_partial_then_complete_matches_python(tmp_path: Path) -> None:
    root = _cache_fixture(tmp_path)
    python_first = _python(root, tmp_path, work_limit=1)
    rust_first = _rust(
        root, census_state=tmp_path / "rust-census.json", work_limit=1
    )
    assert rust_first.returncode == 0, rust_first.stderr
    assert _scrub(json.loads(rust_first.stdout)) == _scrub(python_first)
    assert _rows(python_first)[0]["cache_status"] == "partial"
    assert _rows(python_first)[0]["verdict"] == "BLOCKED"

    python_second = _python(root, tmp_path)
    rust_second = _rust(root, census_state=tmp_path / "rust-census.json")
    assert rust_second.returncode == 0, rust_second.stderr
    assert _scrub(json.loads(rust_second.stdout)) == _scrub(python_second)
    assert _rows(python_second)[0]["cache_status"] == "complete"
    assert _rows(python_second)[0]["verdict"] == "DELETABLE"
    rust_bytes = _rows(json.loads(rust_second.stdout))[0]["cache_bytes"]
    python_bytes = _rows(python_second)[0]["cache_bytes"]
    assert isinstance(rust_bytes, int) and isinstance(python_bytes, int)
    assert rust_bytes > 30 * 1024
    assert abs(rust_bytes - python_bytes) <= 4096


def test_audit_census_flat_cache_small_limit_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    config = root / ".wrkslots.yml"
    config.write_text(config.read_text() + "cache_globs:\n  - cache\n")
    cache = root / "worktrees" / "slot-a" / "repo" / "cache"
    cache.mkdir(parents=True)
    for index in range(200):
        (cache / f"f{index}.bin").write_bytes(b"x" * 64)
    python_payload = _python(root, tmp_path, work_limit=5)
    rust_result = _rust(
        root, census_state=tmp_path / "rust-census.json", work_limit=5
    )
    assert rust_result.returncode == 0, rust_result.stderr
    assert _scrub(json.loads(rust_result.stdout)) == _scrub(python_payload)
    assert _rows(python_payload)[0]["cache_status"] == "partial"


def test_audit_census_symlinked_candidate_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    config = root / ".wrkslots.yml"
    config.write_text(config.read_text() + "cache_globs:\n  - cache\n")
    checkout = root / "worktrees" / "slot-a" / "repo"
    real = checkout / "real-cache"
    real.mkdir()
    (real / "f.bin").write_bytes(b"x" * 4096)
    (checkout / "cache").symlink_to(real, target_is_directory=True)
    actual = _check(root, tmp_path)
    row = _rows(actual)[0]
    assert row["cache_status"] == "error"
    assert "cache path crosses a symlink" in str(row["cache_error"])


def test_audit_census_nested_git_matches_python(tmp_path: Path) -> None:
    root = _cache_fixture(tmp_path)
    nested = root / "worktrees" / "slot-a" / "repo" / "cache" / "nested" / ".git"
    nested.mkdir(parents=True)
    (nested / "HEAD").write_text("ref: refs/heads/main\n")
    actual = _check(root, tmp_path)
    row = _rows(actual)[0]
    assert row["cache_status"] == "error"
    assert row["verdict"] == "BLOCKED"
    assert "nested Git metadata" in str(row["cache_error"])


def test_audit_census_state_inside_project_refuses(tmp_path: Path) -> None:
    root = _cache_fixture(tmp_path)
    result = _rust(root, census_state=root / "census.json")
    assert result.returncode != 0
    assert "outside the managed project" in result.stderr
    args = argparse.Namespace(
        project_root=str(root), machine=None, format="json", gate=False,
        cache_census_state=str(root / "census.json"),
        cache_work_limit=100000, cache_wall_seconds=60.0, wait_lock=0.0,
    )
    with pytest.raises(wrkslots_cli.Refusal):
        wrkslots_cli._cmd_audit(args)


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


def _validate_fixture(base: Path) -> Path:
    root = _fixture(base)
    upstream = base / "upstream-repo"
    subprocess.run(
        ["git", "-C", str(upstream), "worktree", "remove", "--force",
         str(root / "worktrees" / "slot-a" / "repo")],
        capture_output=True, check=False,
    )
    subprocess.run(
        ["rm", "-rf", str(root / "worktrees" / "slot-a")], check=True
    )
    _git(
        upstream, "worktree", "add", "-q", "-b", "suite/validate",
        str(root / "worktrees" / "validate" / "slot-a" / "repo"), "HEAD",
    )
    active_path = root / "worktrees" / "ACTIVE.node-a.json"
    doc = json.loads(active_path.read_text())
    record = doc["slots"][0]
    record["slot_type"] = "validate"
    record["agent"] = "validate-slot-a"
    record["checkouts"][0]["path"] = "worktrees/validate/slot-a/repo"
    record["checkouts"][0]["branch"] = "suite/validate"
    active_path.write_text(json.dumps(doc))
    return root


def test_audit_validate_deletable_matches_python(tmp_path: Path) -> None:
    root = _validate_fixture(tmp_path)
    actual = _check(root, tmp_path)
    rows = _rows(actual)
    assert len(rows) == 1
    assert rows[0]["verdict"] == "DELETABLE"
    assert rows[0]["liveness_state"] == "dead"


def test_audit_validate_absent_matches_python(tmp_path: Path) -> None:
    root = _validate_fixture(tmp_path)
    subprocess.run(
        ["rm", "-rf", str(root / "worktrees" / "validate" / "slot-a")],
        check=True,
    )
    _git(tmp_path / "upstream-repo", "worktree", "prune")
    actual = _check(root, tmp_path)
    rows = _rows(actual)
    assert rows[0]["verdict"] == "RECOVERABLE"
    assert "recover-absent-validate-rows" in str(rows[0]["reasons"])


def test_audit_validate_foreign_machine_matches_python(tmp_path: Path) -> None:
    root = _validate_fixture(tmp_path)
    _git(
        tmp_path / "upstream-repo", "worktree", "add", "-q", "-b",
        "suite/validate-v", str(root / "worktrees" / "validate" / "slot-v" / "repo"),
        "HEAD",
    )
    active_path = root / "worktrees" / "ACTIVE.node-a.json"
    record = json.loads(active_path.read_text())["slots"][0]
    foreign = json.loads(json.dumps(record))
    foreign["slot"] = "slot-v"
    foreign["agent"] = "validate-slot-v"
    foreign["machine"] = "node-b"
    foreign["checkouts"][0]["path"] = "worktrees/validate/slot-v/repo"
    foreign["checkouts"][0]["branch"] = "suite/validate-v"
    (root / "worktrees" / "ACTIVE.node-b.json").write_text(
        json.dumps(
            {"schema": 2, "machine": "node-b", "revision": 7, "slots": [foreign]}
        )
    )
    (root / "worktrees" / "ARCHIVED.node-b.json").write_text(
        json.dumps({"schema": 2, "machine": "node-b", "revision": 3, "records": []})
    )
    actual = _check(root, tmp_path)
    by_slot = {str(row["slot"]): row for row in _rows(actual)}
    assert by_slot["slot-v"]["liveness_state"] == "unverifiable"
    assert by_slot["slot-v"]["verdict"] == "BLOCKED"


def _write_run_handle(
    root: Path,
    payload: dict[str, object] | str,
    *,
    name: str = "slot-a-run.json",
) -> Path:
    runs = root / "ignored" / "validate" / "runs"
    runs.mkdir(parents=True, exist_ok=True)
    path = runs / name
    path.write_text(payload if isinstance(payload, str) else json.dumps(payload))
    return path


def _handle_identity(proc: subprocess.Popen[bytes]) -> dict[str, object]:
    identity = wrkslots_cli._read_process_identity(proc.pid)
    return {
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
    }


def _dead_handle_identity() -> dict[str, object]:
    proc = subprocess.Popen(["sleep", "30"])
    try:
        return _handle_identity(proc)
    finally:
        proc.kill()
        proc.wait()


def test_audit_validate_unrelated_run_handle_matches_python(tmp_path: Path) -> None:
    root = _validate_fixture(tmp_path)
    _write_run_handle(
        root,
        {"checkout": "/elsewhere", "unit": "some-unit.service"},
        name="some-unit.json",
    )
    actual = _check(root, tmp_path)
    assert _rows(actual)[0]["verdict"] == "DELETABLE"


def test_audit_validate_related_run_handle_matches_python(tmp_path: Path) -> None:
    root = _validate_fixture(tmp_path)
    checkout = root / "worktrees" / "validate" / "slot-a" / "repo"
    _write_run_handle(
        root,
        {"checkout": str(checkout), "unit": "slot-a-run.service"},
    )
    actual = _check(root, tmp_path)
    row = _rows(actual)[0]
    assert row["verdict"] == "DELETABLE"
    assert row["liveness_state"] == "dead"


def test_audit_validate_relative_run_handle_matches_python(tmp_path: Path) -> None:
    root = _validate_fixture(tmp_path)
    _write_run_handle(
        root,
        {
            "checkout": "worktrees/validate/slot-a/repo",
            "unit": "slot-a-run.service",
        },
    )
    actual = _check(root, tmp_path)
    assert _rows(actual)[0]["verdict"] == "DELETABLE"


def test_audit_validate_source_run_handle_matches_python(tmp_path: Path) -> None:
    root = _validate_fixture(tmp_path)
    checkout = root / "worktrees" / "validate" / "slot-a" / "repo"
    _write_run_handle(
        root,
        {
            "source_checkout": str(checkout / "nested"),
            "unit": "slot-a-run.service",
        },
    )
    actual = _check(root, tmp_path)
    assert _rows(actual)[0]["verdict"] == "DELETABLE"


def test_audit_validate_dead_process_run_handle_matches_python(
    tmp_path: Path,
) -> None:
    root = _validate_fixture(tmp_path)
    checkout = root / "worktrees" / "validate" / "slot-a" / "repo"
    _write_run_handle(
        root,
        {
            "checkout": str(checkout),
            "unit": "slot-a-run.service",
            "process_identity": _dead_handle_identity(),
        },
    )
    actual = _check(root, tmp_path)
    assert _rows(actual)[0]["verdict"] == "DELETABLE"


def test_audit_validate_live_process_run_handle_matches_python(
    tmp_path: Path,
) -> None:
    root = _validate_fixture(tmp_path)
    checkout = root / "worktrees" / "validate" / "slot-a" / "repo"
    proc = subprocess.Popen(["sleep", "30"])
    try:
        _write_run_handle(
            root,
            {
                "checkout": str(checkout),
                "unit": "slot-a-run.service",
                "process_identity": _handle_identity(proc),
            },
        )
        actual = _check(root, tmp_path)
    finally:
        proc.kill()
        proc.wait()
    row = _rows(actual)[0]
    assert row["verdict"] == "BLOCKED"
    assert row["liveness_state"] == "alive"


def test_audit_validate_other_boot_run_handle_matches_python(
    tmp_path: Path,
) -> None:
    root = _validate_fixture(tmp_path)
    checkout = root / "worktrees" / "validate" / "slot-a" / "repo"
    proc = subprocess.Popen(["sleep", "30"])
    try:
        identity = _handle_identity(proc)
        identity["boot_id"] = "foreign-boot"
        _write_run_handle(
            root,
            {
                "checkout": str(checkout),
                "unit": "slot-a-run.service",
                "process_identity": identity,
            },
        )
        actual = _check(root, tmp_path)
    finally:
        proc.kill()
        proc.wait()
    assert _rows(actual)[0]["verdict"] == "DELETABLE"


def test_audit_validate_duplicate_run_handle_matches_python(
    tmp_path: Path,
) -> None:
    root = _validate_fixture(tmp_path)
    checkout = root / "worktrees" / "validate" / "slot-a" / "repo"
    _write_run_handle(
        root,
        (
            '{"checkout":"/elsewhere","checkout":"'
            + str(checkout)
            + '","unit":"slot-a-run.service"}'
        ),
    )
    actual = _check(root, tmp_path)
    row = _rows(actual)[0]
    assert row["verdict"] == "BLOCKED"
    assert row["liveness_state"] == "unverifiable"


def test_audit_validate_mismatched_run_handle_matches_python(
    tmp_path: Path,
) -> None:
    root = _validate_fixture(tmp_path)
    checkout = root / "worktrees" / "validate" / "slot-a" / "repo"
    _write_run_handle(
        root,
        {"checkout": str(checkout), "unit": "slot-a-run.service"},
        name="other.json",
    )
    actual = _check(root, tmp_path)
    row = _rows(actual)[0]
    assert row["verdict"] == "BLOCKED"
    assert row["liveness_state"] == "unverifiable"


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
