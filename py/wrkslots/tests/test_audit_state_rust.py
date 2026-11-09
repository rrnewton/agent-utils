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
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import cast

import pytest

from wrkslots import cli as wrkslots_cli

from wrkslots.tests.rust_binary import rust_wrkslotsd

# Registry judgment, not process evidence: see the pinned_process_evidence fixture.
pytestmark = pytest.mark.usefixtures("pinned_process_evidence")


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
    entries = payload.get("non_slot_entries")
    if isinstance(entries, list):
        for entry in entries:
            if isinstance(entry, dict) and isinstance(entry.get("bytes"), int):
                entry["bytes"] = "counted"
    return payload


def _python(
    root: Path,
    tmp_path: Path,
    *,
    work_limit: int = 100000,
    state_name: str = "census-state.json",
    wall_seconds: float = 60.0,
    plan_total_seconds: float = 30.0,
) -> dict[str, object]:
    args = argparse.Namespace(
        project_root=str(root), machine=None, format="json", gate=False,
        cache_census_state=str(tmp_path / state_name),
        cache_work_limit=work_limit, cache_wall_seconds=wall_seconds,
        cache_plan_total_seconds=plan_total_seconds,
        wait_lock=0.0,
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
    wall_seconds: float = 60.0,
    plan_total_seconds: float | None = None,
) -> subprocess.CompletedProcess[str]:
    command = [str(rust_wrkslotsd()), "parse-audit", "--root", str(root)]
    if census_state is not None:
        command += ["--cache-census-state", str(census_state)]
    if work_limit is not None:
        command += [
            "--cache-work-limit", str(work_limit),
            "--cache-wall-seconds", str(wall_seconds),
        ]
    if plan_total_seconds is not None:
        command += ["--cache-plan-total-seconds", str(plan_total_seconds)]
    return subprocess.run(
        command,
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def _rows(payload: dict[str, object]) -> list[dict[str, object]]:
    return cast(list[dict[str, object]], payload["slots"])


def _check(root: Path, tmp_path: Path) -> dict[str, object]:
    # Both editions read live host evidence (the process table
    # and the user-systemd population) between the fixture's
    # writes, and that evidence churns on a shared host: the
    # oracle's settled snapshot refuses after three unstable
    # enumerations while a single listing still answers, so a
    # pair read across a churn burst legitimately disagrees
    # about liveness. Retry the pair until both editions read
    # the same host state; judgment differences survive every
    # retry and still fail.
    actual: dict[str, object] = {}
    expected: dict[str, object] = {}
    for attempt in range(5):
        result = _rust(root, census_state=tmp_path / f"rust-census-{attempt}.json")
        assert result.returncode == 0, result.stderr
        actual = _scrub(json.loads(result.stdout))
        expected = _scrub(
            _python(root, tmp_path, state_name=f"census-state-{attempt}.json")
        )
        if actual == expected:
            return actual
        time.sleep(1.0)
    assert actual == expected
    return actual


def test_audit_nested_cargo_target_matches_python(tmp_path: Path) -> None:
    """#1042: a cargo target nested below the checkout is counted by both."""

    root = _fixture(tmp_path)
    target = root / "worktrees" / "slot-a" / "repo" / "temp" / "cargo-target"
    (target / ".fingerprint").mkdir(parents=True)
    (target / "CACHEDIR.TAG").write_bytes(
        wrkslots_cli._CACHEDIR_TAG_SIGNATURE + b"\n"
    )
    (target / "artifact").write_bytes(b"x" * 4096)
    actual = _check(root, tmp_path)
    row = _rows(actual)[0]
    assert row["cache_status"] == "complete"
    assert row["cache_bytes"] == "counted"


# Unpinned: the real census path stays exercised in both editions.
@pytest.mark.real_process_evidence
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
    assert row["verdict"] == "DELETABLE"
    assert str(row["size_unknown_reason"]).endswith(
        "; rerun audit with a larger --cache-work-limit for a complete total"
    )


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
    first = _rows(python_first)[0]
    assert first["cache_status"] == "partial"
    # #1106: an allowance shortfall leaves the size unknown, not the verdict.
    assert first["verdict"] == "DELETABLE"
    assert first["reasons"] == []
    assert first["size_known"] is False
    assert str(first["size_unknown_reason"]).startswith("the bounded cache census is partial")

    python_second = _python(root, tmp_path)
    rust_second = _rust(root, census_state=tmp_path / "rust-census.json")
    assert rust_second.returncode == 0, rust_second.stderr
    assert _scrub(json.loads(rust_second.stdout)) == _scrub(python_second)
    assert _rows(python_second)[0]["cache_status"] == "complete"
    assert _rows(python_second)[0]["verdict"] == "DELETABLE"
    assert _rows(python_second)[0]["size_known"] is True
    assert _rows(python_second)[0]["size_unknown_reason"] is None
    rust_bytes = _rows(json.loads(rust_second.stdout))[0]["cache_bytes"]
    python_bytes = _rows(python_second)[0]["cache_bytes"]
    assert isinstance(rust_bytes, int) and isinstance(python_bytes, int)
    assert rust_bytes > 30 * 1024
    assert abs(rust_bytes - python_bytes) <= 4096


def test_audit_live_lane_exhausted_cache_not_unknown_matches_python(
    tmp_path: Path,
) -> None:
    # Mirrors test_lifecycle's regression (33582924): a BLOCKED lane
    # whose liveness probe proves it alive stays BLOCKED even when
    # the bounded cache census fails; it must not be reclassified
    # UNKNOWN, in either edition.
    root = _cache_fixture(tmp_path)
    config = root / ".wrkslots.yml"
    config.write_text(config.read_text().replace("- cache\n", "- cache/**\n"))
    (root / "live.py").write_text(
        "import sys\n"
        "print('agent=agent-slot-a rc=1 state: alive')\n"
        "sys.exit(1)\n"
    )
    python_payload = _python(root, tmp_path, work_limit=5)
    rust_result = _rust(
        root, census_state=tmp_path / "rust-census.json", work_limit=5
    )
    assert rust_result.returncode == 0, rust_result.stderr
    assert _scrub(json.loads(rust_result.stdout)) == _scrub(python_payload)
    row = _rows(python_payload)[0]
    assert row["verdict"] == "BLOCKED"
    assert row["liveness_state"] == "alive"
    assert row["cache_error"] is not None
    assert python_payload["unknown_slots"] == []
    assert python_payload["state"] == "ok"


def _state_without_mac(path: Path) -> dict[str, object]:
    payload = json.loads(path.read_text())
    assert isinstance(payload, dict)
    mac = payload.pop("hmac_sha256", None)
    assert isinstance(mac, str) and len(mac) == 64
    return payload


def test_audit_census_resume_state_matches_python(tmp_path: Path) -> None:
    root = _cache_fixture(tmp_path)
    shared = tmp_path / "shared-census.json"
    python_first = _python(root, tmp_path, work_limit=10, state_name=shared.name)
    python_state = _state_without_mac(shared)
    shared.unlink()
    rust_first = _rust(root, census_state=shared, work_limit=10)
    assert rust_first.returncode == 0, rust_first.stderr
    assert _scrub(json.loads(rust_first.stdout)) == _scrub(python_first)
    assert _rows(python_first)[0]["cache_status"] == "partial"
    # The persisted progress is byte-identical apart from the MAC:
    # same root key, phase, cursors, and counters.
    assert _state_without_mac(shared) == python_state
    roots = python_state["roots"]
    assert isinstance(roots, dict) and len(roots) == 1
    record = next(iter(roots.values()))
    assert isinstance(record, dict)
    assert record["phase"] == "measure"
    assert isinstance(record["entries_visited"], int)
    assert record["entries_visited"] > 0

    # A second small-limit run resumes instead of restarting:
    # progress accumulates beyond the first run's counters.
    python_second = _python(root, tmp_path, work_limit=10, state_name=shared.name)
    assert _rows(python_second)[0]["cache_status"] == "partial"
    rust_second = _rust(root, census_state=shared, work_limit=10)
    assert rust_second.returncode == 0, rust_second.stderr
    assert _scrub(json.loads(rust_second.stdout)) == _scrub(python_second)
    state_after = _state_without_mac(shared)
    roots_after = state_after["roots"]
    assert isinstance(roots_after, dict)
    record_after = next(iter(roots_after.values()))
    assert isinstance(record_after, dict)
    total_after = int(record_after["entries_visited"]) + int(
        record_after["entries_verified"]
    )
    total_before = int(record["entries_visited"]) + int(record["entries_verified"])
    assert total_after > total_before

    # Either edition can finish what the other started.
    rust_third = _rust(root, census_state=shared)
    assert rust_third.returncode == 0, rust_third.stderr
    assert _rows(json.loads(rust_third.stdout))[0]["cache_status"] == "complete"
    python_third = _python(root, tmp_path, state_name=shared.name)
    assert _scrub(json.loads(rust_third.stdout)) == _scrub(python_third)
    assert _rows(python_third)[0]["cache_status"] == "complete"


def test_audit_census_finalize_reserve_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    config = root / ".wrkslots.yml"
    config.write_text(config.read_text() + "cache_globs:\n  - cache\n")
    cache = root / "worktrees" / "slot-a" / "repo" / "cache"
    cache.mkdir(parents=True)
    files = 20
    for index in range(files):
        (cache / f"f{index}.bin").write_bytes(b"x" * 64)
    # Measure plus verify of the flat cache costs exactly
    # 2 * (files + dot entries); stage finalize with nothing left.
    staged_limit = 2 * (files + 2)
    python_first = _python(root, tmp_path, work_limit=staged_limit)
    rust_first = _rust(
        root, census_state=tmp_path / "rust-census.json", work_limit=staged_limit
    )
    assert rust_first.returncode == 0, rust_first.stderr
    assert _scrub(json.loads(rust_first.stdout)) == _scrub(python_first)
    assert _rows(python_first)[0]["cache_status"] == "partial"

    python_second = _python(root, tmp_path, work_limit=1)
    rust_second = _rust(root, census_state=tmp_path / "rust-census.json", work_limit=1)
    assert rust_second.returncode == 0, rust_second.stderr
    assert _scrub(json.loads(rust_second.stdout)) == _scrub(python_second)
    row = _rows(python_second)[0]
    assert row["cache_status"] == "error"
    assert "fresh cache finalization requires" in str(row["cache_error"])


def test_audit_census_corrupt_state_is_ignored(tmp_path: Path) -> None:
    root = _cache_fixture(tmp_path)
    python_first = _python(root, tmp_path, work_limit=10)
    assert _rows(python_first)[0]["cache_status"] == "partial"
    state = tmp_path / "rust-census.json"
    rust_first = _rust(root, census_state=state, work_limit=10)
    assert rust_first.returncode == 0, rust_first.stderr
    assert _rows(json.loads(rust_first.stdout))[0]["cache_status"] == "partial"
    # Corrupting either edition's state makes the next run start
    # fresh: partial again under the same small limit, never an
    # audit failure.
    state.write_bytes(b"{not json")
    rust_second = _rust(root, census_state=state, work_limit=10)
    assert rust_second.returncode == 0, rust_second.stderr
    assert _rows(json.loads(rust_second.stdout))[0]["cache_status"] == "partial"
    python_state = tmp_path / "census-state.json"
    python_state.write_bytes(b"{not json")
    python_second = _python(root, tmp_path, work_limit=10)
    assert _rows(python_second)[0]["cache_status"] == "partial"


def test_audit_census_wall_timeout_matches_python(tmp_path: Path) -> None:
    root = _cache_fixture(tmp_path)
    # Same live-evidence retry as `_check`: a pair read across
    # a host churn burst can disagree about liveness, not law.
    python_payload: dict[str, object] = {}
    rust_payload: dict[str, object] = {}
    for attempt in range(5):
        python_payload = _python(
            root, tmp_path, wall_seconds=0.000001,
            state_name=f"census-state-{attempt}.json",
        )
        rust_result = _rust(
            root, census_state=tmp_path / f"rust-census-{attempt}.json",
            work_limit=100000, wall_seconds=0.000001,
        )
        assert rust_result.returncode == 0, rust_result.stderr
        rust_payload = json.loads(rust_result.stdout)
        if _scrub(rust_payload) == _scrub(python_payload):
            break
        time.sleep(1.0)
    assert _scrub(rust_payload) == _scrub(python_payload)
    row = _rows(python_payload)[0]
    assert row["cache_status"] == "error"
    assert "fixed wall allowance" in str(row["cache_error"])
    # #1106: the wall allowance, the one that depends on host load, no longer
    # decides the verdict.
    assert row["verdict"] == "DELETABLE"
    assert row["size_known"] is False
    assert str(row["size_unknown_reason"]).endswith(
        "; rerun audit with a larger --cache-wall-seconds for a complete total"
    )


def test_audit_planning_total_matches_python(tmp_path: Path) -> None:
    # The audit-wide planning total, not the subject's own wall allowance, runs
    # out: both editions spell that exhaustion the same way.
    root = _cache_fixture(tmp_path)
    python_payload: dict[str, object] = {}
    rust_payload: dict[str, object] = {}
    for attempt in range(5):
        python_payload = _python(
            root, tmp_path, plan_total_seconds=0.000001,
            state_name=f"census-state-{attempt}.json",
        )
        rust_result = _rust(
            root, census_state=tmp_path / f"rust-census-{attempt}.json",
            work_limit=100000, plan_total_seconds=0.000001,
        )
        assert rust_result.returncode == 0, rust_result.stderr
        rust_payload = json.loads(rust_result.stdout)
        if _scrub(rust_payload) == _scrub(python_payload):
            break
        time.sleep(1.0)
    assert _scrub(rust_payload) == _scrub(python_payload)
    row = _rows(python_payload)[0]
    assert row["cache_status"] == "error"
    assert "total planning allowance of 1e-06 seconds before this subject's fixed wall " \
        "allowance of 60 seconds" in str(row["cache_error"])
    assert row["verdict"] == "DELETABLE"
    assert str(row["size_unknown_reason"]).endswith(
        "; rerun audit with a larger --cache-plan-total-seconds for a complete total"
    )


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
    # #1107: the gate cannot tell this expired row's reclaim state, so its one
    # verdict is UNKNOWN, as unknown_slots says.
    assert row["verdict"] == "UNKNOWN"
    assert actual["unknown_slots"] == ["slot-a"]
    assert "nested Git metadata" in str(row["cache_error"])
    # #1106: what the census found still decides; it is no size shortfall.
    assert row["size_known"] is False
    assert row["size_unknown_reason"] is None


def _validate_cache_fixture(base: Path) -> tuple[Path, Path]:
    root = _validate_fixture(base)
    config = root / ".wrkslots.yml"
    config.write_text(config.read_text() + "cache_globs:\n  - cache\n")
    cache = root / "worktrees" / "validate" / "slot-a" / "repo" / "cache"
    cache.mkdir(parents=True)
    (cache / "f.bin").write_bytes(b"x" * 1024)
    return root, cache


def test_audit_validate_nested_repo_disposable_matches_python(
    tmp_path: Path,
) -> None:
    root, cache = _validate_cache_fixture(tmp_path)
    _repo(cache / "nested")
    actual = _check(root, tmp_path)
    row = _rows(actual)[0]
    assert row["cache_status"] == "complete"
    assert row["cache_error"] is None
    assert row["verdict"] == "DELETABLE"


def test_audit_validate_nested_core_worktree_matches_python(
    tmp_path: Path,
) -> None:
    root, cache = _validate_cache_fixture(tmp_path)
    _repo(cache / "nested")
    _git(cache / "nested", "config", "core.worktree", "/tmp/elsewhere")
    actual = _check(root, tmp_path)
    row = _rows(actual)[0]
    assert row["cache_status"] == "error"
    assert "repository config sets core.worktree" in str(row["cache_error"])


def test_audit_validate_nested_gitfile_matches_python(
    tmp_path: Path,
) -> None:
    root, cache = _validate_cache_fixture(tmp_path)
    nested = cache / "nested"
    nested.mkdir(parents=True)
    (nested / ".git").write_text("gitdir: /tmp/elsewhere\n")
    actual = _check(root, tmp_path)
    row = _rows(actual)[0]
    assert row["cache_status"] == "error"
    assert ".git is a file" in str(row["cache_error"])


def test_audit_validate_nested_linked_worktrees_matches_python(
    tmp_path: Path,
) -> None:
    root, cache = _validate_cache_fixture(tmp_path)
    _repo(cache / "nested")
    (cache / "nested" / ".git" / "worktrees" / "fake").mkdir(parents=True)
    actual = _check(root, tmp_path)
    row = _rows(actual)[0]
    assert row["cache_status"] == "error"
    assert "registered linked worktrees: fake" in str(row["cache_error"])


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


_SCOPED_CREATE_NAME = "CREATE.6.node-a.6.slot-j.journal"


def _scoped_create_journal(base: Path) -> dict[str, object]:
    head = _git(base / "upstream-repo", "rev-parse", "HEAD").strip()
    return {
        "schema": 2,
        "kind": "create",
        "machine": "node-a",
        "slot": "slot-j",
        "agent": "agent-j",
        "task": "scoped work",
        "purpose": "testing",
        "created_at": "2026-10-26T00:00:00Z",
        "heartbeat_at": "2026-10-26T01:00:00Z",
        "planned": [
            {
                "name": "repo",
                "destination": "worktrees/slot-j/repo",
                "repository": "../upstream-repo",
                "branch": "suite/j",
                "start_point": head,
                "remote": "origin",
                "remote_url_sha256": "0" * 64,
                "landed_ref": "refs/remotes/origin/main",
            }
        ],
        "created": [],
        "post_provision_hooks": [],
        "heartbeat_ttl_seconds": 60,
        "owner": None,
        "coordinator_lease": {
            "pid": 4242,
            "start_ticks": 100,
            "boot_id": "boot-1",
            "host_id": "host-a",
            "cgroup_path": "/init.scope",
        },
    }


def _append_event(root: Path, kind: str, payload: dict[str, object]) -> None:
    config = wrkslots_cli._load_config(str(root), None)
    wrkslots_cli._ensure_event_log(config, "node-a")
    wrkslots_cli._write_event_file(config, "node-a", kind, payload)


def _python_refusal(root: Path, tmp_path: Path) -> str:
    with pytest.raises(Exception) as caught:
        _python(root, tmp_path)
    return str(caught.value)


def test_audit_scoped_journal_without_evidence_refuses(
    tmp_path: Path,
) -> None:
    root = _fixture(tmp_path)
    (root / "worktrees" / _SCOPED_CREATE_NAME).write_text(
        json.dumps(_scoped_create_journal(tmp_path))
    )
    message = "scoped create journal has no matching append-only progress evidence"
    assert message in _python_refusal(root, tmp_path)
    result = _rust(root, census_state=tmp_path / "rust-census.json")
    assert result.returncode != 0
    assert message in result.stderr


def test_audit_scoped_journal_with_progress_matches_python(
    tmp_path: Path,
) -> None:
    root = _fixture(tmp_path)
    journal = _scoped_create_journal(tmp_path)
    (root / "worktrees" / _SCOPED_CREATE_NAME).write_text(json.dumps(journal))
    _append_event(
        root,
        "operation-progress-recorded",
        {
            "operation": "create",
            "slot": "slot-j",
            "journal_path": _SCOPED_CREATE_NAME,
            "journal": journal,
        },
    )
    actual = _check(root, tmp_path)
    by_slot = {str(row["slot"]): row for row in _rows(actual)}
    assert by_slot["slot-j"]["verdict"] == "BLOCKED"
    assert by_slot["slot-j"]["owner_state"] == "create-journal"


def test_audit_scoped_journal_differs_from_evidence_refuses(
    tmp_path: Path,
) -> None:
    root = _fixture(tmp_path)
    journal = _scoped_create_journal(tmp_path)
    (root / "worktrees" / _SCOPED_CREATE_NAME).write_text(json.dumps(journal))
    recorded = _scoped_create_journal(tmp_path)
    recorded["task"] = "other work"
    _append_event(
        root,
        "operation-progress-recorded",
        {
            "operation": "create",
            "slot": "slot-j",
            "journal_path": _SCOPED_CREATE_NAME,
            "journal": recorded,
        },
    )
    message = "scoped create journal differs from append-only progress evidence"
    assert message in _python_refusal(root, tmp_path)
    result = _rust(root, census_state=tmp_path / "rust-census.json")
    assert result.returncode != 0
    assert message in result.stderr


def test_audit_scoped_journal_cache_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    config = root / ".wrkslots.yml"
    config.write_text(config.read_text() + "cache_globs:\n  - cache\n")
    head = _git(tmp_path / "upstream-repo", "rev-parse", "HEAD").strip()
    checkout = root / "worktrees" / "slot-j" / "repo"
    _git(
        tmp_path / "upstream-repo", "worktree", "add", "-q", "-b", "suite/j",
        str(checkout), "HEAD",
    )
    cache = checkout / "cache"
    cache.mkdir(parents=True)
    for index in range(5):
        (cache / f"f{index}.bin").write_bytes(b"x" * 1024)
    journal = _scoped_create_journal(tmp_path)
    journal["created"] = [
        {
            "name": "repo",
            "path": "worktrees/slot-j/repo",
            "repository": "../upstream-repo",
            "branch": "suite/j",
            "start_point": head,
            "remote": "origin",
            "remote_url_sha256": "0" * 64,
            "landed_ref": "refs/remotes/origin/main",
            "head": head,
            "containing_remote_refs": [],
            "vcs": "git",
        }
    ]
    (root / "worktrees" / _SCOPED_CREATE_NAME).write_text(json.dumps(journal))
    _append_event(
        root,
        "operation-progress-recorded",
        {
            "operation": "create",
            "slot": "slot-j",
            "journal_path": _SCOPED_CREATE_NAME,
            "journal": journal,
        },
    )
    actual = _check(root, tmp_path)
    by_slot = {str(row["slot"]): row for row in _rows(actual)}
    assert by_slot["slot-j"]["cache_status"] == "complete"
    assert by_slot["slot-j"]["cache_error"] is None
    assert by_slot["slot-j"]["cache_bytes"] == "counted"


def _slot_j_worktree(tmp_path: Path, root: Path) -> tuple[str, Path]:
    head = _git(tmp_path / "upstream-repo", "rev-parse", "HEAD").strip()
    checkout = root / "worktrees" / "slot-j" / "repo"
    _git(
        tmp_path / "upstream-repo", "worktree", "add", "-q", "-b", "suite/j",
        str(checkout), "HEAD",
    )
    return head, checkout


def _created_entry(head: str) -> dict[str, object]:
    return {
        "name": "repo",
        "path": "worktrees/slot-j/repo",
        "repository": "../upstream-repo",
        "branch": "suite/j",
        "start_point": head,
        "remote": "origin",
        "remote_url_sha256": "0" * 64,
        "landed_ref": "refs/remotes/origin/main",
        "head": head,
        "containing_remote_refs": [],
        "vcs": "git",
    }


def test_audit_scoped_journal_missing_checkout_matches_python(
    tmp_path: Path,
) -> None:
    root = _fixture(tmp_path)
    head, checkout = _slot_j_worktree(tmp_path, root)
    journal = _scoped_create_journal(tmp_path)
    journal["created"] = [_created_entry(head)]
    (root / "worktrees" / _SCOPED_CREATE_NAME).write_text(json.dumps(journal))
    _append_event(
        root,
        "operation-progress-recorded",
        {
            "operation": "create",
            "slot": "slot-j",
            "journal_path": _SCOPED_CREATE_NAME,
            "journal": journal,
        },
    )
    _git(tmp_path / "upstream-repo", "worktree", "remove", str(checkout))
    actual = _check(root, tmp_path)
    by_slot = {str(row["slot"]): row for row in _rows(actual)}
    assert by_slot["slot-j"]["cache_status"] == "complete"
    assert by_slot["slot-j"]["cache_error"] is None


def test_audit_scoped_journal_registered_missing_refuses(
    tmp_path: Path,
) -> None:
    root = _fixture(tmp_path)
    head, checkout = _slot_j_worktree(tmp_path, root)
    journal = _scoped_create_journal(tmp_path)
    journal["created"] = [_created_entry(head)]
    (root / "worktrees" / _SCOPED_CREATE_NAME).write_text(json.dumps(journal))
    _append_event(
        root,
        "operation-progress-recorded",
        {
            "operation": "create",
            "slot": "slot-j",
            "journal_path": _SCOPED_CREATE_NAME,
            "journal": journal,
        },
    )
    shutil.rmtree(checkout)
    message = "journaled checkout repo is missing but Git still registers it"
    assert message in _python_refusal(root, tmp_path)
    result = _rust(root, census_state=tmp_path / "rust-census.json")
    assert result.returncode != 0
    assert message in result.stderr


def test_audit_legacy_journal_differs_from_evidence_refuses(
    tmp_path: Path,
) -> None:
    root = _fixture(tmp_path)
    journal = _scoped_create_journal(tmp_path)
    (root / "worktrees" / "ACTIVE.node-a.journal").write_text(
        json.dumps(journal)
    )
    recorded = _scoped_create_journal(tmp_path)
    recorded["task"] = "other work"
    _append_event(
        root,
        "operation-progress-recorded",
        {"journal": recorded},
    )
    message = (
        "standalone recovery journal differs from "
        "append-only operation evidence"
    )
    assert message in _python_refusal(root, tmp_path)
    result = _rust(root, census_state=tmp_path / "rust-census.json")
    assert result.returncode != 0
    assert message in result.stderr


def _churn_hook(
    tmp_path: Path, tag: str, slot: Path, *, fire_at: int = 1
) -> tuple[str, Path]:
    """A process to vanish, and a hook that kills it and starts one in ``slot``.

    Each scan runs the hook once it has listed /proc (WRKSLOTS_TEST_CENSUS_CHURN);
    on its ``fire_at``-th call the listed process is killed, reaped by the
    subreaper, and a new process whose working directory is inside the slot
    (or its fenced name, after a removal's path fence) starts, which that
    listing never saw.  Returns the hook command and the file
    the appearing process's PID is written to.
    """

    work = tmp_path / f"churn-{tag}"
    work.mkdir()
    vanishing = subprocess.run(
        ["/bin/sh", "-c", "setsid sleep 300 </dev/null >/dev/null 2>&1 & echo $!"],
        capture_output=True, text=True, check=True, cwd=work, timeout=30,
    ).stdout.strip()
    appeared = work / "appeared.pid"
    calls = work / "calls"
    hook = (
        f"n=$(($(cat {calls} 2>/dev/null || echo 0) + 1)); echo $n > {calls}; "
        f"[ $n -eq {fire_at} ] || exit 0; "
        f"kill -KILL {vanishing}; "
        f"n=0; while kill -0 {vanishing} 2>/dev/null && [ $n -lt 500 ]; do sleep 0.01; n=$((n+1)); done; "
        # After a removal's path fence the checkout lives under a fenced name.
        f"d={slot}; [ -d $d ] || d=$(find {slot.parent.parent} -maxdepth 4 -type d "
        f"-name {slot.name} 2>/dev/null | head -n 1); cd \"$d\" || exit 1; "
        f"setsid sleep 300 </dev/null >/dev/null 2>&1 & echo $! > {appeared}"
    )
    return hook, appeared


def _seam_calls(hook_counter: Path) -> int:
    return int(hook_counter.read_text()) if hook_counter.exists() else 0


def _stop_appeared(appeared: Path) -> str:
    """Kill the process the hook started, after checking it is that sleep; return its PID."""

    pid = appeared.read_text().strip()
    cmdline = Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
    if cmdline[:2] == [b"sleep", b"300"]:
        os.kill(int(pid), 9)
    return pid


def _churn_row(row: dict[str, object], pid: str) -> dict[str, object]:
    reasons = [str(reason).replace(pid, "<appeared>") for reason in cast(list[object], row["reasons"])]
    return {**row, "reasons": reasons}


@pytest.mark.real_process_evidence
def test_audit_census_sees_appearing_pid_and_tolerates_vanished_pid(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Python: a process that vanished mid-census is no refusal; one that appeared is seen (#1102)."""

    root = _fixture(tmp_path)
    hook, appeared = _churn_hook(tmp_path, "python", root / "worktrees" / "slot-a" / "repo")
    monkeypatch.setenv("WRKSLOTS_TEST_CENSUS_CHURN", hook)
    try:
        payload = _python(root, tmp_path)
    finally:
        pid = _stop_appeared(appeared)
    row = _rows(payload)[0]
    reasons = [str(reason) for reason in cast(list[object], row["reasons"])]
    assert row["verdict"] == "BLOCKED", row
    assert any(pid in reason for reason in reasons), reasons
    assert not any("changed during" in reason for reason in reasons), reasons


@pytest.mark.real_process_evidence
def test_audit_forced_churn_matches_python(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Both editions, one injected vanishing PID and one appearing PID: the same row (#1102).

    The editions word a slot user differently (a link path against the kind of
    reference), so the reasons are compared for naming the appearing process.
    """

    root = _fixture(tmp_path)
    slot = root / "worktrees" / "slot-a"
    rows: list[dict[str, object]] = []
    for edition in ("python", "rust"):
        hook, appeared = _churn_hook(tmp_path, edition, slot / "repo")
        monkeypatch.setenv("WRKSLOTS_TEST_CENSUS_CHURN", hook)
        try:
            if edition == "python":
                payload = _scrub(_python(root, tmp_path))
            else:
                result = _rust(root, census_state=tmp_path / "rust-census.json")
                assert result.returncode == 0, result.stderr
                payload = _scrub(json.loads(result.stdout))
        finally:
            pid = _stop_appeared(appeared)
        row = _rows(payload)[0]
        reasons = [str(reason) for reason in cast(list[object], row["reasons"])]
        assert row["verdict"] == "BLOCKED", (edition, row)
        assert len(reasons) == 1, (edition, reasons)
        assert reasons[0].startswith(f"live process {pid} uses slot {slot}"), (edition, reasons)
        rows.append({**row, "reasons": None})
    assert rows[0] == rows[1]


# The leader blocks SIGUSR1 before it starts the thread (which inherits the
# mask), waits for it, and calls pthread_exit; the thread changes into the
# slot first and prints its thread ID.
_PTHREAD_EXIT_CHILD = """
import ctypes, os, signal, sys, threading, time

signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGUSR1})

def hold():
    os.chdir(sys.argv[1])
    print(threading.get_native_id(), flush=True)
    while True:
        time.sleep(60)

threading.Thread(target=hold, daemon=True).start()
signal.sigwait({signal.SIGUSR1})
ctypes.CDLL(None).pthread_exit(None)
"""


@pytest.mark.real_process_evidence
def test_audit_follows_a_leader_that_exits_after_the_listing_like_python(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The lead's required case, both editions: pthread_exit under a thread in the slot BLOCKS (#1102).

    The census lists /proc, then the seam makes the listed leader call
    pthread_exit, so its own entries show nothing while its thread's working
    directory is inside the slot.  Python used to discard the attempt for it;
    Rust never looked behind a zombie leader and published the row DELETABLE.
    Both now read the thread at /proc/PID/task/TID and name it.
    """

    root = _fixture(tmp_path)
    slot = root / "worktrees" / "slot-a"
    rows: list[dict[str, object]] = []
    for edition in ("python", "rust"):
        with subprocess.Popen(
            [sys.executable, "-I", "-c", _PTHREAD_EXIT_CHILD, str(slot / "repo")],
            stdout=subprocess.PIPE, text=True, cwd="/",
        ) as child:
            try:
                assert child.stdout is not None
                thread = int(child.stdout.readline())
                calls = tmp_path / f"exit-calls-{edition}"
                monkeypatch.setenv(
                    "WRKSLOTS_TEST_CENSUS_CHURN",
                    f"n=$(($(cat {calls} 2>/dev/null || echo 0) + 1)); echo $n > {calls}; "
                    f"[ $n -eq 1 ] || exit 0; kill -USR1 {child.pid}; "
                    f"n=0; until grep -q '^State:[[:space:]]*Z' /proc/{child.pid}/status "
                    f"|| [ $n -ge 500 ]; do sleep 0.01; n=$((n+1)); done",
                )
                if edition == "python":
                    payload = _scrub(_python(root, tmp_path))
                else:
                    result = _rust(root, census_state=tmp_path / "rust-census.json")
                    assert result.returncode == 0, result.stderr
                    payload = _scrub(json.loads(result.stdout))
                # The seam fired, so the leader exited after the listing.  A
                # census attempt discarded for other host motion runs the seam
                # again, a no-op past its first call.
                assert int(calls.read_text()) >= 1, edition
            finally:
                child.kill()
                child.wait(timeout=10)
        row = _rows(payload)[0]
        reasons = [str(reason) for reason in cast(list[object], row["reasons"])]
        assert row["verdict"] == "BLOCKED", (edition, row)
        assert len(reasons) == 1, (edition, reasons)
        assert reasons[0].startswith(
            f"live thread {thread} of process {child.pid} (whose main thread has exited) "
            f"uses slot {slot}"
        ), (edition, reasons)
        rows.append({**row, "reasons": None})
    assert rows[0] == rows[1]


def test_pinned_process_evidence_fails_loudly_when_a_process_uses_the_slot(
    tmp_path: Path,
) -> None:
    """The pin never hides real use: a pinned test whose slot is in use fails, naming the process."""

    slot = tmp_path / "slot"
    slot.mkdir()
    process = subprocess.Popen(["sleep", "60"], cwd=slot)
    try:
        with pytest.raises(pytest.fail.Exception, match="mark it real_process_evidence") as caught:
            wrkslots_cli._capture_process_path_census([slot])
        assert f"PID {process.pid} cwd -> {slot}" in str(caught.value)
    finally:
        process.kill()
        process.wait()


def _many_slot_fixture(base: Path, count: int) -> Path:
    """The single-slot fixture grown to ``count`` registered, removable slots."""

    root = _fixture(base)
    upstream = base / "upstream-repo"
    active = root / "worktrees" / "ACTIVE.node-a.json"
    document = json.loads(active.read_text())
    template = document["slots"][0]
    refspecs = []
    for index in range(2, count + 1):
        slot = f"slot-{index:02d}"
        branch = f"suite/work-{index:02d}"
        _git(
            upstream, "worktree", "add", "-q", "-b", branch,
            str(root / "worktrees" / slot / "repo"), "HEAD",
        )
        refspecs.append(branch)
        record = json.loads(json.dumps(template))
        record["slot"] = slot
        record["agent"] = f"agent-{slot}"
        record["checkouts"][0]["path"] = f"worktrees/{slot}/repo"
        record["checkouts"][0]["branch"] = branch
        document["slots"].append(record)
    _git(upstream, "push", "-q", "origin", *refspecs)
    active.write_text(json.dumps(document))
    return root


def _heavy_churn_hook(base: Path, tag: str, slot: Path, *, vanishing: int) -> tuple[str, Path]:
    """Like _churn_hook, at scale: ``vanishing`` listed processes die, as many unrelated ones start.

    One more process starts inside ``slot``.  The unrelated births are short-lived,
    so some also vanish while a scan reads them.
    """

    work = base / f"churn-{tag}"
    work.mkdir()
    pids = subprocess.run(
        [
            "/bin/sh", "-c",
            f"for i in $(seq {vanishing}); do setsid sleep 300 </dev/null >/dev/null 2>&1 & "
            "echo $!; done",
        ],
        capture_output=True, text=True, check=True, cwd=work, timeout=60,
    ).stdout.split()
    appeared = work / "appeared.pid"
    script = work / "hook.sh"
    script.write_text(
        f"[ -e {work}/fired ] && exit 0; : > {work}/fired\n"
        f"kill -KILL {' '.join(pids)}\n"
        f"for i in $(seq {vanishing}); do setsid sleep 0.$((i % 9 + 1)) </dev/null >/dev/null 2>&1 & done\n"
        f"cd {slot} || exit 1\n"
        f"setsid sleep 300 </dev/null >/dev/null 2>&1 & echo $! > {appeared}\n"
    )
    return f"/bin/sh {script}", appeared


@pytest.mark.real_process_evidence
def test_audit_heavy_churn_many_slots_matches_python(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """#1102 at a consumer's scale: 77 slots, 60 listed processes vanishing, 60 unrelated
    births and one process appearing in one slot, all between a listing and its reads.

    Both editions publish 76 slots DELETABLE and the used one BLOCKED, naming the
    appearing process; neither refuses for churn (no UNKNOWN).
    """

    root = _many_slot_fixture(tmp_path, 77)
    used = root / "worktrees" / "slot-40"
    verdicts: list[dict[str, object]] = []
    for edition in ("python", "rust"):
        hook, appeared = _heavy_churn_hook(tmp_path, edition, used / "repo", vanishing=60)
        monkeypatch.setenv("WRKSLOTS_TEST_CENSUS_CHURN", hook)
        try:
            if edition == "python":
                payload = _python(root, tmp_path)
            else:
                result = _rust(root, census_state=tmp_path / "rust-census.json")
                assert result.returncode == 0, result.stderr
                payload = json.loads(result.stdout)
        finally:
            pid = _stop_appeared(appeared)
        rows = {str(row["slot"]): row for row in _rows(payload)}
        assert len(rows) == 77, edition
        unknown = cast(list[object], payload["unknown_slots"])
        # A refusal names its cause in the row reasons; show one, so a gate
        # log says why the census refused, not only that it did (#1102).
        assert unknown == [], (
            edition, unknown, rows[str(unknown[0])]["reasons"] if unknown else None
        )
        blocked = rows.pop("slot-40")
        assert blocked["verdict"] == "BLOCKED", (edition, blocked)
        reasons = cast(list[object], blocked["reasons"])
        assert any(str(reason).startswith(f"live process {pid} uses slot {used}") for reason in reasons), (
            edition, reasons,
        )
        assert {str(row["verdict"]) for row in rows.values()} == {"DELETABLE"}, edition
        verdicts.append({slot: row["verdict"] for slot, row in rows.items()})
    assert verdicts[0] == verdicts[1]


def _holder_replacement_hook(base: Path, tag: str, slot: Path) -> tuple[str, Path, Path]:
    """A process working in ``slot`` now, and a hook that replaces it there once.

    The holder is detached, so the subreaper reaps it once killed (a child of
    this test would stay a zombie with its generation intact).  The hook kills
    it, waits until it is gone, and starts a new process in the slot, whose PID
    it writes to the returned file.  Returns the hook, that file, and the file
    naming the holder.
    """

    work = base / f"holder-{tag}"
    work.mkdir()
    holder_file = work / "holder.pid"
    holder_file.write_text(
        subprocess.run(
            ["/bin/sh", "-c", "setsid sleep 300 </dev/null >/dev/null 2>&1 & echo $!"],
            capture_output=True, text=True, check=True, cwd=slot, timeout=30,
        ).stdout.strip()
    )
    holder = holder_file.read_text()
    appeared = work / "appeared.pid"
    hook = (
        f"[ -e {work}/fired ] && exit 0; : > {work}/fired; "
        f"kill -KILL {holder}; "
        f"n=0; while [ -e /proc/{holder}/stat ] && [ $n -lt 500 ]; do sleep 0.01; n=$((n+1)); done; "
        f"cd {slot} || exit 1; setsid sleep 300 </dev/null >/dev/null 2>&1 & echo $! > {appeared}"
    )
    return hook, appeared, holder_file


@pytest.mark.real_process_evidence
def test_audit_follows_a_matched_holder_replaced_before_its_recheck(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """#1102: the holder find matched is gone at its identity recheck, and a new one works there.

    Before, the census refused ("PID ... identity changed after privileged find
    matched a selected path; the selected path may still be in use") and every
    candidate row became UNKNOWN.  Now the gone generation is dropped and the
    next listing reads its replacement, so the row is BLOCKED by the new process.
    The Rust edition has no separate matcher step; its seam replaces the holder
    between its listing and its reads, the same history at its own boundary.
    """

    root = _fixture(tmp_path)
    slot = root / "worktrees" / "slot-a"
    for edition, variable in (
        ("python", "WRKSLOTS_TEST_CENSUS_MATCH_CHURN"),
        ("rust", "WRKSLOTS_TEST_CENSUS_CHURN"),
    ):
        hook, appeared, holder_file = _holder_replacement_hook(tmp_path, edition, slot / "repo")
        monkeypatch.setenv(variable, hook)
        try:
            if edition == "python":
                payload = _python(root, tmp_path)
            else:
                result = _rust(root, census_state=tmp_path / "rust-census.json")
                assert result.returncode == 0, result.stderr
                payload = json.loads(result.stdout)
        finally:
            monkeypatch.delenv(variable)
            holder = int(holder_file.read_text())
            with contextlib.suppress(OSError):
                if Path(f"/proc/{holder}/cmdline").read_bytes().split(b"\0")[:2] == [
                    b"sleep", b"300",
                ]:
                    os.kill(holder, 9)
            pid = _stop_appeared(appeared)
        row = _rows(payload)[0]
        reasons = [str(reason) for reason in cast(list[object], row["reasons"])]
        assert row["verdict"] == "BLOCKED", (edition, row)
        assert payload["unknown_slots"] == [], (edition, reasons)
        assert not any("identity changed" in reason for reason in reasons), (edition, reasons)
        assert any(reason.startswith(f"live process {pid} uses slot {slot}") for reason in reasons), (
            edition, reasons,
        )


def test_audit_size_unknown_text_matches_python(tmp_path: Path) -> None:
    """#1106: both editions print 'DELETABLE (size unknown: ...)' for an allowance shortfall."""

    root = _cache_fixture(tmp_path)
    args = argparse.Namespace(
        project_root=str(root), machine=None, format="human", gate=False,
        cache_census_state=str(tmp_path / "python-census.json"),
        cache_work_limit=1, cache_wall_seconds=60.0, cache_plan_total_seconds=30.0,
        wait_lock=0.0,
    )
    stream = io.StringIO()
    with contextlib.redirect_stdout(stream):
        assert wrkslots_cli._cmd_audit(args) == 0
    rust = subprocess.run(
        [
            str(rust_wrkslotsd()), "audit-text", "--root", str(root),
            "--cache-census-state", str(tmp_path / "rust-census.json"),
            "--cache-work-limit", "1", "--cache-wall-seconds", "60",
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )
    assert rust.returncode == 0, rust.stderr

    def slot_lines(text: str) -> list[str]:
        return [line for line in text.splitlines() if ": slot-a: " in line]

    expected = (
        "DELETABLE (size unknown: the bounded cache census is partial; rerun audit, which "
        "resumes it from its --cache-census-state, or raise --cache-work-limit and "
        "--cache-wall-seconds, for a complete total): slot-a: all removal proofs pass"
    )
    assert slot_lines(stream.getvalue()) == [expected]
    assert slot_lines(rust.stdout) == [expected]


def _text_audits(root: Path, tmp_path: Path) -> tuple[str, str]:
    """Both editions' human audit output for one fixture."""

    args = argparse.Namespace(
        project_root=str(root), machine=None, format="human", gate=False,
        cache_census_state=str(tmp_path / "python-text-census.json"),
        cache_work_limit=100000, cache_wall_seconds=60.0, cache_plan_total_seconds=30.0,
        wait_lock=0.0,
    )
    stream = io.StringIO()
    with contextlib.redirect_stdout(stream):
        assert wrkslots_cli._cmd_audit(args) == 0
    rust = subprocess.run(
        [
            str(rust_wrkslotsd()), "audit-text", "--root", str(root),
            "--cache-census-state", str(tmp_path / "rust-text-census.json"),
            "--cache-wall-seconds", "60",
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )
    assert rust.returncode == 0, rust.stderr
    return stream.getvalue(), rust.stdout


def test_audit_text_labels_match_the_json_verdicts(tmp_path: Path) -> None:
    """#1107: one verdict per row, the same in both formats and both editions."""

    root = _cache_fixture(tmp_path)
    nested = root / "worktrees" / "slot-a" / "repo" / "cache" / "nested" / ".git"
    nested.mkdir(parents=True)
    (nested / "HEAD").write_text("ref: refs/heads/main\n")
    payload = _check(root, tmp_path)
    labels = {str(row["slot"]): str(row["verdict"]) for row in _rows(payload)}
    assert labels == {"slot-a": "UNKNOWN"}
    for text in _text_audits(root, tmp_path):
        printed = {
            line.split(": ")[1]: line.split(": ")[0].split(" (")[0]
            for line in text.splitlines()
            if line.count(": ") >= 2 and not line.startswith(" ")
        }
        assert printed == labels, text


def test_audit_non_slot_entries_match_python(tmp_path: Path) -> None:
    """#1113: names no slot can have are listed once, never as unregistered slots."""

    root = _fixture(tmp_path)
    worktrees = root / "worktrees"
    (worktrees / ".pytest_cache" / "v" / "cache").mkdir(parents=True)
    (worktrees / ".pytest_cache" / "v" / "cache" / "lastfailed").write_text("{}\n")
    (worktrees / "_scratch").mkdir()
    (worktrees / "_scratch" / "notes.txt").write_text("x" * 5000)
    (worktrees / "stray").mkdir()
    payload = _check(root, tmp_path)
    rows = {str(row["slot"]): row for row in _rows(payload)}
    assert set(rows) == {"slot-a", "stray"}
    assert rows["stray"]["owner_state"] == "unregistered"
    assert payload["non_slot_entries"] == [
        {
            "slot_type": "agent", "name": ".pytest_cache",
            "path": str(worktrees / ".pytest_cache"), "bytes": "counted",
        },
        {
            "slot_type": "agent", "name": "_scratch",
            "path": str(worktrees / "_scratch"), "bytes": "counted",
        },
    ]
    python_text, rust_text = _text_audits(root, tmp_path)

    def section(text: str) -> list[str]:
        lines = text.splitlines()
        start = lines.index(
            "non-slot entries in the slots root (no slot can have these names; "
            "inspect or delete them):"
        )
        return lines[start:start + 3]

    assert section(python_text) == section(rust_text)
    assert section(python_text)[1].startswith("  agent .pytest_cache: ")
    assert section(python_text)[2].startswith("  agent _scratch: ")
    assert section(python_text)[2].endswith(" bytes")

