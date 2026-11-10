"""Differential: wrkslotsd remove against the Python remove mutation.

#562 rust-primary-wrkslots slice 36, the eleventh writer slice. The
Python oracle is `_cmd_remove` on a copy of the fixture registry;
the Rust writer runs on the original. Resulting ACTIVE/ARCHIVED
records and event kinds must match apart from wall-clock stamps
and each fixture's own Git history. Salvage corners needing a
push, a bundle, owner consent, or attestation stay Python and are
refused explicitly by the Rust writer. Temporary fixture
registries only: writers are never pointed at a live registry
before the edition cutover.
"""

from __future__ import annotations

import argparse
import contextlib
import difflib
import hashlib
import io
import json
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import cast

import re
import shutil
from collections.abc import Iterator
from contextlib import contextmanager

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.rust_binary import rust_wrkslotsd


AU_ROOT = Path(__file__).resolve().parents[3]


def _git(cwd: Path, *args: str) -> str:
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(
        {
            "LC_ALL": "C",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_SYSTEM": "/dev/null",
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
    _git(upstream, "remote", "add", "origin", str(upstream))
    _git(upstream, "fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*")
    _git(
        upstream, "worktree", "add", "-q", "-b", "suite/work",
        str(root / "worktrees" / "slot-a" / "repo"), "HEAD",
    )
    dead = _dead_identity()
    record: dict[str, object] = {
        "slot": "slot-a",
        "agent": "agent-slot-a",
        "task": "do work",
        "purpose": "testing",
        "machine": "node-a",
        "generation": 1,
        "created_at": "2026-10-26T00:00:00Z",
        "heartbeat_at": "2026-10-26T01:00:00Z",
        "heartbeat_ttl_seconds": 60,
        "owner": dead,
        "coordinator_lease": dead,
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
                "remote_url_sha256": hashlib.sha256(
                    str(upstream).encode()
                ).hexdigest(),
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


def _pair(tmp_path: Path) -> tuple[Path, Path]:
    return _fixture(tmp_path / "rust"), _fixture(tmp_path / "oracle")


@contextmanager
def _own_fs(tmp_path: Path) -> Iterator[Path]:
    """A fixture base on a different filesystem device than tmp_path.

    #651: census-sensitive tests must not share the /tmp device with
    unrelated host processes, whose unreadable descriptors would
    otherwise refuse the census as indeterminate. Production slots
    are separate filesystem images; /dev/shm is the test stand-in.
    Skips with a clear reason when no distinct device is available.
    """
    shm = Path("/dev/shm")
    if not shm.is_dir() or os.stat(shm).st_dev == os.stat(tmp_path).st_dev:
        pytest.skip(
            "no distinct-device fixture filesystem available: census tests "
            "need e.g. /dev/shm on a different device than the pytest tmp dir"
        )
    base = shm / f"wrkslots-remove-{os.getpid()}-{tmp_path.name}"
    base.mkdir(parents=True, exist_ok=True)
    try:
        yield base
    finally:
        shutil.rmtree(base, ignore_errors=True)


def _rust_remove(root: Path, *, fixture: bool = True) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    return subprocess.run(
        [
            str(rust_wrkslotsd()), "remove", "slot-a",
            "--coordinator-pid", str(os.getpid()), "--expected-generation", "1",
            "--root", str(root),
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env
    )


#: The item budget an in-process test remove runs under (#1195
#: reclaim-test-budget-sibling): far above a loaded gate host's /proc census
#: (about 48 s each), so a test of what remove does is not refused for host
#: speed. It keeps the default single-item path, unlike --no-lock-budget.
#: Tests of the budget itself pass ``budget_seconds=None`` and set their own.
_TEST_REMOVE_ITEM_SECONDS = 3600.0


def _python_remove(
    root: Path,
    *,
    no_lock_budget: bool = False,
    budget_seconds: float | None = _TEST_REMOVE_ITEM_SECONDS,
) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot="slot-a",
        coordinator_authorized=False, validate_complete=False,
        coordinator_pid=os.getpid(), expected_generation=1,
        salvage_archive_root=None, validation_proof_manifest=None,
        completed_record=None, attest_agent_stopped=None,
        attest_agent_never_started=None, attest_note=None,
        archive_drifted_checkouts=False, adopt_legacy_change=False, wait_lock=0.0,
        no_lock_budget=no_lock_budget,
    )
    if budget_seconds is None:
        return wrkslots_cli._cmd_remove(args)
    production = wrkslots_cli._AGENT_REMOVE_BATCH_ITEM_SECONDS
    wrkslots_cli._AGENT_REMOVE_BATCH_ITEM_SECONDS = budget_seconds
    try:
        return wrkslots_cli._cmd_remove(args)
    finally:
        wrkslots_cli._AGENT_REMOVE_BATCH_ITEM_SECONDS = production


def _scrub_doc(doc: dict[str, object]) -> dict[str, object]:
    for record in cast(list[dict[str, object]], doc.get("slots", doc.get("records", []))):
        handoff = record.get("handoff")
        if isinstance(handoff, dict):
            handoff.pop("recorded_at")
        for checkout in cast(list[dict[str, object]], record.get("checkouts", [])):
            checkout["head"] = "HEAD"
            checkout["start_point"] = "HEAD"
            checkout["remote_url_sha256"] = "URLSHA"
        record.pop("finished_at", None)
        record.pop("archive_id", None)
        if isinstance(record.get("validation"), list):
            record["validation"] = [
                re.sub(r"[0-9a-f]{40}", "HEAD", str(entry))
                for entry in cast(list[object], record["validation"])
            ]
        handoff_value = record.get("handoff")
        if isinstance(handoff_value, dict) and isinstance(
            handoff_value.get("validation"), list
        ):
            handoff_value["validation"] = [
                re.sub(r"[0-9a-f]{40}", "HEAD", str(entry))
                for entry in cast(list[object], handoff_value["validation"])
            ]
        for item in cast(list[dict[str, object]], record.get("salvage", [])):
            item["source_head"] = "HEAD"
            item["salvage_commit"] = "HEAD"
    return doc


def _doc(root: Path, name: str) -> dict[str, object]:
    doc: dict[str, object] = json.loads((root / "worktrees" / name).read_text())
    return _scrub_doc(doc)


def _kinds_and_actions(root: Path) -> list[tuple[str, str]]:
    directory = root / "worktrees" / "EVENTS.node-a"
    found: list[tuple[str, str]] = []
    if not directory.is_dir():
        return found
    for path in sorted(directory.iterdir()):
        event: dict[str, object] = json.loads(path.read_text())
        assert event["sha256"] == wrkslots_cli._event_digest(
            {k: v for k, v in event.items() if k != "sha256"}
        )
        payload = cast(dict[str, object], event["payload"])
        found.append((str(event["kind"]), str(payload.get("action", ""))))
    return found


def test_remove_matches_python(tmp_path: Path) -> None:
    with _own_fs(tmp_path) as base:
        _remove_matches_python(base)


def _remove_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    assert _python_remove(oracle_root) == 0
    result = _rust_remove(rust_root)
    assert result.returncode == 0, result.stderr
    outcome = json.loads(result.stdout)
    assert outcome["outcome"] == "removed and archived"
    assert _doc(rust_root, "ACTIVE.node-a.json") == _doc(oracle_root, "ACTIVE.node-a.json")
    assert _doc(rust_root, "ARCHIVED.node-a.json") == _doc(
        oracle_root, "ARCHIVED.node-a.json"
    )
    assert _kinds_and_actions(rust_root) == _kinds_and_actions(oracle_root)
    for root in (rust_root, oracle_root):
        assert not (root / "worktrees" / "slot-a").exists()
        assert not (root / "worktrees" / "ACTIVE.node-a.journal").exists()


def _identity_obj(pid: int) -> dict[str, object]:
    identity = wrkslots_cli._read_process_identity(pid)
    return {
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    }


_HANDOFF_CHILD = """
import argparse, json, os, sys, time
sys.path.insert(0, sys.argv[1])
from pathlib import Path
from wrkslots import cli
root, source, done, mode = sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5]
assert cli._cmd_write_handoff(argparse.Namespace(
    project_root=root, machine=None, slot="slot-a", agent="agent-slot-a",
    owner_pid=os.getpid(), expected_generation=1, from_file=source,
    wait_lock=0.0)) == 0
if mode == "read":
    assert cli._cmd_read_handoff(argparse.Namespace(
        project_root=root, machine=None, slot="slot-a",
        coordinator_pid=os.getpid(), coordinator_authorized=True,
        adopt_legacy_change=False, expected_generation=None,
        wait_lock=0.0)) == 0
Path(done).write_text("done")
time.sleep(60)
"""


def _write_and_read_handoff(root: Path, source_dir: Path, *, read: bool) -> None:
    """Give slot-a a handoff sidecar via its owner process, then kill it.

    The owner must be the invoking process for write-handoff, so a
    child Python process owns the slot, writes (and optionally reads)
    the handoff as itself, signals completion, and is then killed so
    removal sees a proven-dead owner.
    """
    source_dir.mkdir(parents=True, exist_ok=True)
    source = source_dir / "notes.md"
    source.write_text("# Handoff\n\nAll done.\n")
    done = source_dir / "done"
    env = dict(os.environ)
    env["PYTHONPATH"] = str(AU_ROOT / "py")
    proc = subprocess.Popen(
        [
            sys.executable, "-c", _HANDOFF_CHILD, str(AU_ROOT / "py"),
            str(root), str(source), str(done), "read" if read else "write",
        ],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    try:
        active_path = root / "worktrees" / "ACTIVE.node-a.json"
        doc = json.loads(active_path.read_text())
        doc["slots"][0]["owner"] = _identity_obj(proc.pid)
        doc["slots"][0]["coordinator_lease"] = _identity_obj(proc.pid)
        active_path.write_text(json.dumps(doc))
        for _ in range(300):
            if done.exists():
                break
            if proc.poll() is not None:
                _out, err = proc.communicate()
                raise AssertionError(f"handoff child failed: {err}")
            time.sleep(0.1)
        assert done.exists(), "handoff child did not finish"
    finally:
        proc.kill()
        proc.wait()


def test_remove_read_sidecar_retires_it(tmp_path: Path) -> None:
    with _own_fs(tmp_path) as base:
        _remove_read_sidecar_retires_it(base)


def _remove_read_sidecar_retires_it(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    _write_and_read_handoff(oracle_root, tmp_path / "oracle-src", read=True)
    _write_and_read_handoff(rust_root, tmp_path / "rust-src", read=True)
    assert _python_remove(oracle_root) == 0
    result = _rust_remove(rust_root)
    assert result.returncode == 0, result.stderr
    assert _doc(rust_root, "ACTIVE.node-a.json") == _doc(oracle_root, "ACTIVE.node-a.json")
    assert _doc(rust_root, "ARCHIVED.node-a.json") == _doc(
        oracle_root, "ARCHIVED.node-a.json"
    )
    assert _kinds_and_actions(rust_root) == _kinds_and_actions(oracle_root)
    retired_pattern = re.compile(
        r"HANDOFF-RETIRED\.[0-9a-f]{16}\.[0-9a-f]+\.[0-9a-f]+\."
        r"[0-9a-f]{64}\.[0-9a-f]{64}\.json"
    )
    for root in (rust_root, oracle_root):
        control = root / "worktrees"
        assert not (control / "HANDOFF.6.node-a.6.slot-a.1.json").exists()
        retired = [
            path for path in control.iterdir() if retired_pattern.fullmatch(path.name)
        ]
        assert len(retired) == 1
        assert not (root / "worktrees" / "slot-a").exists()


def test_remove_unread_sidecar_refuses_like_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    _write_and_read_handoff(oracle_root, tmp_path / "oracle-src", read=False)
    _write_and_read_handoff(rust_root, tmp_path / "rust-src", read=False)
    with pytest.raises(Exception) as caught:
        _python_remove(oracle_root)
    assert "contains an unread HANDOFF.md or handoff sidecar" in str(caught.value)
    result = _rust_remove(rust_root)
    assert result.returncode == 1
    assert "contains an unread HANDOFF.md or handoff sidecar" in result.stderr
    assert (rust_root / "worktrees" / "slot-a").exists()


def test_remove_sidecar_without_intent_refuses_like_python(
    tmp_path: Path,
) -> None:
    # A real read sidecar whose contents were replaced afterwards: it
    # no longer byte-matches its owner-authenticated write intent, so
    # the publication gate refuses before any read-provenance credit.
    rust_root, oracle_root = _pair(tmp_path)
    _write_and_read_handoff(oracle_root, tmp_path / "oracle-src", read=True)
    _write_and_read_handoff(rust_root, tmp_path / "rust-src", read=True)
    for root in (rust_root, oracle_root):
        sidecar_path = root / "worktrees" / "HANDOFF.6.node-a.6.slot-a.1.json"
        sidecar = json.loads(sidecar_path.read_text())
        # recorded_at is part of the intent-bound artifact object but
        # is not otherwise constrained by the sidecar loader, so this
        # replacement reaches the publication gate itself.
        sidecar["recorded_at"] = "2026-10-26T09:09:09Z"
        sidecar_path.write_text(json.dumps(sidecar))
    with pytest.raises(Exception) as caught:
        _python_remove(oracle_root)
    assert "no matching owner-authenticated publication intent" in str(caught.value)
    result = _rust_remove(rust_root)
    assert result.returncode == 1
    assert "no matching owner-authenticated publication intent" in result.stderr
    assert (rust_root / "worktrees" / "slot-a").exists()


def test_remove_with_image_records_matches_python(tmp_path: Path) -> None:
    with _own_fs(tmp_path) as base:
        _remove_with_image_records(base)


def _remove_with_image_records(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    for root in (rust_root, oracle_root):
        control = root / "worktrees"
        active_path = control / "ACTIVE.node-a.json"
        doc = json.loads(active_path.read_text())
        record = json.loads(json.dumps(doc["slots"][0]))
        record["slot"] = "slot-img"
        record["agent"] = "agent-slot-img"
        record["checkouts"][0]["path"] = "worktrees/slot-img/repo"
        doc["slots"].append(record)
        active_path.write_text(json.dumps(doc))
        (control / "slot-img").mkdir()
        image_dir = control / "slot-images" / "agent" / "slot-img"
        image_dir.mkdir(parents=True)
        (image_dir / "slot.img").write_bytes(b"")
        (image_dir / "IMAGE.json").write_text(
            json.dumps(
                {
                    "schema": "wrkslots-slot-image/v1",
                    "slot": "slot-img",
                    "slot_type": "agent",
                    "phase": "ready",
                    "backend": "kernel",
                    "location": str(control / "slot-img"),
                    "created_at": "2026-10-26T00:00:00Z",
                    "ceiling_bytes": 1024,
                    "state_ceiling_bytes": 1024,
                }
            )
        )
    assert _python_remove(oracle_root) == 0
    result = _rust_remove(rust_root)
    assert result.returncode == 0, result.stderr
    remaining = _doc(rust_root, "ACTIVE.node-a.json")
    remaining_slots = cast(list[dict[str, object]], remaining["slots"])
    assert [record["slot"] for record in remaining_slots] == ["slot-img"]


def _add_cache_globs(root: Path) -> Path:
    config = root / ".wrkslots.yml"
    config.write_text(config.read_text() + "cache_globs:\n  - cache-dir\n")
    cache = root / "worktrees" / "slot-a" / "repo" / "cache-dir"
    cache.mkdir(parents=True)
    (cache / "junk.txt").write_text("regenerable\n")
    return cache


def test_remove_with_cache_globs_matches_python(tmp_path: Path) -> None:
    with _own_fs(tmp_path) as base:
        _remove_with_cache_globs(base)


def _remove_with_cache_globs(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    _add_cache_globs(rust_root)
    _add_cache_globs(oracle_root)
    assert _python_remove(oracle_root) == 0
    result = _rust_remove(rust_root)
    assert result.returncode == 0, result.stderr
    assert _doc(rust_root, "ACTIVE.node-a.json") == _doc(oracle_root, "ACTIVE.node-a.json")
    assert _doc(rust_root, "ARCHIVED.node-a.json") == _doc(
        oracle_root, "ARCHIVED.node-a.json"
    )
    assert not (rust_root / "worktrees" / "slot-a").exists()


def test_remove_cache_nested_repo_refuses(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    cache = _add_cache_globs(rust_root)
    _repo(cache / "inner")
    result = _rust_remove(rust_root)
    assert result.returncode == 1
    assert "nested Git repository" in result.stderr
    assert (rust_root / "worktrees" / "slot-a").exists()


def test_remove_dirty_checkout_refuses_salvage_scope(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    (rust_root / "worktrees" / "slot-a" / "repo" / "notes.txt").write_text("wip\n")
    result = _rust_remove(rust_root)
    assert result.returncode == 1
    assert "salvage stays with the Python edition" in result.stderr
    assert (rust_root / "worktrees" / "slot-a").exists()


def test_remove_unpublished_commit_refuses_salvage_scope(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    checkout = rust_root / "worktrees" / "slot-a" / "repo"
    # Point the checkout at a separate bare remote so a local commit
    # is genuinely unpublished (the fixture's self-remote shares the
    # branch ref with the checkout).
    bare = tmp_path / "bare.git"
    _git(tmp_path, "init", "-q", "--bare", str(bare))
    _git(checkout, "remote", "add", "pub", str(bare))
    _git(checkout, "push", "-q", "pub", "HEAD:main", "HEAD:suite/work")
    _git(checkout, "fetch", "-q", "pub", "+refs/heads/*:refs/remotes/pub/*")
    active_path = rust_root / "worktrees" / "ACTIVE.node-a.json"
    document = json.loads(active_path.read_text())
    record_checkout = document["slots"][0]["checkouts"][0]
    record_checkout["remote"] = "pub"
    record_checkout["landed_ref"] = "refs/remotes/pub/main"
    record_checkout["remote_url_sha256"] = hashlib.sha256(
        str(bare).encode()
    ).hexdigest()
    record_checkout["containing_remote_refs"] = ["refs/remotes/pub/main"]
    active_path.write_text(json.dumps(document))
    (checkout / "file.txt").write_text("two\n")
    _git(checkout, "add", "file.txt")
    _git(checkout, "commit", "-q", "-m", "two")
    result = _rust_remove(rust_root)
    assert result.returncode == 1
    assert "salvage stays with the Python edition" in result.stderr
    assert (rust_root / "worktrees" / "slot-a").exists()


def test_remove_live_owner_refuses_consent_scope(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    active_path = rust_root / "worktrees" / "ACTIVE.node-a.json"
    document = json.loads(active_path.read_text())
    identity = wrkslots_cli._read_process_identity(os.getpid())
    document["slots"][0]["owner"] = {
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    }
    active_path.write_text(json.dumps(document))
    result = _rust_remove(rust_root)
    assert result.returncode == 1
    assert "consent" in result.stderr
    assert (rust_root / "worktrees" / "slot-a").exists()


def test_remove_refuses_without_fixture_attestation(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    result = _rust_remove(rust_root, fixture=False)
    assert result.returncode == 1
    assert "fixture-only" in result.stderr
    assert (rust_root / "worktrees" / "slot-a").exists()
    doc: dict[str, object] = json.loads(
        (rust_root / "worktrees" / "ACTIVE.node-a.json").read_text()
    )
    assert len(cast(list[object], doc["slots"])) == 1


def _rebuild(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            str(rust_wrkslotsd()), "rebuild",
            "--events-dir", str(root / "worktrees" / "EVENTS.node-a"),
            "--index", str(root / "rebuild.sqlite3"),
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def test_remove_event_log_replays_through_rust_reader(tmp_path: Path) -> None:
    with _own_fs(tmp_path) as base:
        _event_log_replays(base)


def _event_log_replays(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    result = _rust_remove(rust_root)
    assert result.returncode == 0, result.stderr
    replay = _rebuild(rust_root)
    assert replay.returncode == 0, replay.stderr


def test_remove_liveness_bound_alive_refuses(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    (rust_root / "live.py").write_text(
        "import sys\n"
        "print('agent=agent-slot-a rc=1', file=sys.stderr)\n"
        "sys.exit(1)\n"
    )
    result = _rust_remove(rust_root)
    assert result.returncode == 1
    assert "reports owner alive" in result.stderr
    assert (rust_root / "worktrees" / "slot-a").exists()


def test_remove_liveness_unbound_exit_one_refuses(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    (rust_root / "live.py").write_text(
        "import sys\n"
        "print('Traceback (most recent call last):', file=sys.stderr)\n"
        "print('RuntimeError: boom', file=sys.stderr)\n"
        "sys.exit(1)\n"
    )
    result = _rust_remove(rust_root)
    assert result.returncode == 1
    assert "unverifiable" in result.stderr
    assert (rust_root / "worktrees" / "slot-a").exists()


def test_remove_liveness_alive_despite_dead_owner_refuses(tmp_path: Path) -> None:
    # The recorded owner process is dead, but the liveness authority
    # still reports the agent alive: Python refuses, and so must Rust.
    rust_root = _fixture(tmp_path / "rust")
    (rust_root / "live.py").write_text(
        "import sys\n"
        "print('agent=agent-slot-a rc=1', file=sys.stderr)\n"
        "sys.exit(1)\n"
    )
    result = _rust_remove(rust_root)
    assert result.returncode == 1
    assert (rust_root / "worktrees" / "slot-a").exists()


PANE_VERDICT_SCRIPT = (
    "import sys\n"
    "print('unverifiable agent=agent-slot-a rc=2 reason=unattributed-panes: "
    "1 live herdr pane(s) run an agent-capable process, but no record "
    "claims them: w1:p9 (bash, codex)', file=sys.stderr)\n"
    "sys.exit(2)\n"
)


def test_remove_unrelated_pane_verdict_removes(tmp_path: Path) -> None:
    """#993: a pane verdict referencing nothing of the slot does not
    veto Rust removal either."""

    rust_root = _fixture(tmp_path / "rust")
    (rust_root / "live.py").write_text(PANE_VERDICT_SCRIPT)
    result = _rust_remove(rust_root)
    assert result.returncode == 0, result.stderr
    assert not (rust_root / "worktrees" / "slot-a").exists()


def test_remove_pane_process_in_slot_refuses(tmp_path: Path) -> None:
    """#993: a live process working inside the slot vetoes Rust
    removal, naming the pid."""

    rust_root = _fixture(tmp_path / "rust")
    (rust_root / "live.py").write_text(PANE_VERDICT_SCRIPT)
    user = subprocess.Popen(
        ["sleep", "60"], cwd=rust_root / "worktrees" / "slot-a" / "repo", text=True
    )
    try:
        result = _rust_remove(rust_root)
    finally:
        user.kill()
        user.wait()
    assert result.returncode == 1, result.stderr
    assert str(user.pid) in result.stderr
    assert (rust_root / "worktrees" / "slot-a").exists()


def test_remove_invalid_sidecar_refuses(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    sidecar = rust_root / "worktrees" / "HANDOFF.6.node-a.6.slot-a.1.json"
    sidecar.write_text("{}\n")
    result = _rust_remove(rust_root)
    assert result.returncode == 1
    assert "sidecar" in result.stderr
    assert (rust_root / "worktrees" / "slot-a").exists()


def test_remove_ignored_file_still_removes(tmp_path: Path) -> None:
    with _own_fs(tmp_path) as base:
        _ignored_file_still_removes(base)


def _ignored_file_still_removes(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    checkout = rust_root / "worktrees" / "slot-a" / "repo"
    (checkout / ".gitignore").write_text("ignored.log\n")
    _git(checkout, "add", ".gitignore")
    _git(checkout, "commit", "-q", "-m", "ignore")
    _git(checkout, "push", "-q", "origin", "HEAD:suite/work")
    (checkout / "ignored.log").write_text("scratch\n")
    result = _rust_remove(rust_root)
    assert result.returncode == 0, result.stderr
    assert not (rust_root / "worktrees" / "slot-a").exists()


def test_remove_forced_churn_refuses_like_python(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """#1102: a process that appears in the slot after the LAST scan listed /proc vetoes removal.

    Both editions, no host load. A first removal of a twin fixture counts the
    scans (WRKSLOTS_TEST_CENSUS_CHURN calls); the second fires on the last one,
    after the path fence, where no later scan would meet the appearing process:
    only the scan's own second listing can.
    """

    from wrkslots.tests.test_audit_state_rust import (
        _churn_hook,
        _seam_calls,
        _stop_appeared,
    )

    def remove(edition: str, root: Path) -> str | None:
        if edition == "python":
            try:
                _python_remove(root)
            except wrkslots_cli.Refusal as exc:
                return str(exc)
            return None
        result = _rust_remove(root)
        return None if result.returncode == 0 else result.stderr

    with _own_fs(tmp_path) as base:
        for edition in ("python", "rust"):
            counting = _fixture(base / f"{edition}-count")
            hook, _appeared = _churn_hook(
                base, f"{edition}-count", counting / "worktrees" / "slot-a" / "repo",
                fire_at=10**6,
            )
            monkeypatch.setenv("WRKSLOTS_TEST_CENSUS_CHURN", hook)
            assert remove(edition, counting) is None, edition
            scans = _seam_calls(base / f"churn-{edition}-count" / "calls")
            assert scans >= 1, edition
            root = _fixture(base / edition)
            hook, appeared = _churn_hook(
                base, edition, root / "worktrees" / "slot-a" / "repo", fire_at=scans
            )
            monkeypatch.setenv("WRKSLOTS_TEST_CENSUS_CHURN", hook)
            try:
                detail = remove(edition, root)
            finally:
                pid = _stop_appeared(appeared)
            assert detail is not None, (edition, "removed a slot in use")
            assert f"live process {pid} uses slot" in detail, (edition, detail)


def _liveness_evidence(root: Path) -> list[dict[str, object]]:
    directory = root / "worktrees" / "EVENTS.node-a"
    found: list[dict[str, object]] = []
    for path in sorted(directory.iterdir()):
        payload = cast(dict[str, object], json.loads(path.read_text())["payload"])
        if payload.get("action") == "registered-liveness-recorded":
            found.append(cast(dict[str, object], payload["evidence"]))
    return found


def test_remove_records_the_same_liveness_line_as_python(tmp_path: Path) -> None:
    """#994: both editions record the dead line escaped, capped and flagged alike."""

    with _own_fs(tmp_path) as base:
        rust_root, oracle_root = _pair(base)
        for root in (rust_root, oracle_root):
            (root / "live.py").write_text(
                "print('dead agent=agent-slot-a rc=0 reason=stopped-by-agentctl "
                "archived=/r/archive/x\\x01\\u00e9 ' + 'y' * 2000)\n"
                "print('second line')\n"
            )
        assert _python_remove(oracle_root) == 0
        result = _rust_remove(rust_root)
        assert result.returncode == 0, result.stderr
        expected = _liveness_evidence(oracle_root)
        assert len(expected) == 1
        assert expected[0]["detail_truncated"] is True
        assert "\\x01\u00e9 yyy" in str(expected[0]["detail"])
        assert _liveness_evidence(rust_root) == expected


# #1121 finish-journal-live-remover: the finish journal names the removing
# process as its top-level owner, so a classifier reads a live removal (one
# deleting its fenced tree outside the lock) as busy, not as interrupted by
# the removed slot's dead owner.
_INTERRUPTED_REMOVE = """
import sys
sys.path.insert(0, sys.argv[1])
from wrkslots import cli
cli._AGENT_REMOVE_BATCH_ITEM_SECONDS = 3600.0  # _TEST_REMOVE_ITEM_SECONDS (#1195)
sys.exit(cli.main([
    "--project-root", sys.argv[2], "remove", "slot-a",
    "--coordinator-pid", sys.argv[3], "--expected-generation", "1",
]))
"""


def _interrupted_python_remove(root: Path) -> tuple[Path, dict[str, object]]:
    """Kill a Python remove of slot-a right after it wrote its finish journal."""

    env = dict(os.environ)
    env["WRKSLOTS_TEST_INTERRUPT"] = "after-finish-journal"
    result = subprocess.run(
        [sys.executable, "-c", _INTERRUPTED_REMOVE, str(AU_ROOT / "py"), str(root), str(os.getpid())],
        capture_output=True, text=True, check=False, timeout=900, env=env,
    )
    assert result.returncode == 86, result.stderr
    path = root / "worktrees" / "ACTIVE.node-a.journal"
    journal: dict[str, object] = json.loads(path.read_text())
    return path, journal


def _classify_both(root: Path) -> tuple[tuple[int, str, str], subprocess.CompletedProcess[str]]:
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        rc = wrkslots_cli.main(
            [
                "--project-root", str(root), "--machine", "node-a",
                "classify-create-journals", "slot-c", "--agent", "agent-other",
                "--slot-type", "agent", "--format", "json",
            ]
        )
    rust = subprocess.run(
        [
            str(rust_wrkslotsd()), "classify-create-journals", "slot-c",
            "--agent", "agent-other", "--slot-type", "agent",
            "--machine", "node-a", "--root", str(root), "--format", "json",
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )
    return (rc, out.getvalue(), err.getvalue()), rust


def test_finish_journal_classifies_by_its_remover_in_both_editions(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "project")
    path, journal = _interrupted_python_remove(root)
    record = cast(dict[str, object], journal["record"])
    remover = cast(dict[str, object], journal["owner"])
    slot_owner = cast(dict[str, object], record["owner"])
    assert remover["pid"] != slot_owner["pid"]
    assert set(remover) == {"pid", "start_ticks", "boot_id", "host_id", "cgroup_path"}

    # The remover was killed: interrupted, as before.
    (rc, _out, err), rust = _classify_both(root)
    assert rc == 3 and "interrupted" in err, err
    # The Rust edition reports this refusal as a host fault, exit 1.
    assert rust.returncode not in (0, 75) and "interrupted" in rust.stderr, rust.stderr

    # A live remover, with the slot's owner dead: busy (75), kind finish.
    # Written as a remover writes it, so the event log carries it too.
    config = wrkslots_cli._load_config(str(root), "node-a")
    journal["owner"] = _identity_obj(os.getpid())
    wrkslots_cli._write_journal(config, journal, journal_path=path)
    (rc, out, err), rust = _classify_both(root)
    assert rc == 75, err
    assert json.loads(out)["busy"]["kind"] == "finish"
    assert json.loads(out)["busy"]["pid"] == os.getpid()
    assert rust.returncode == 75, rust.stderr
    assert json.loads(rust.stdout)["busy"]["kind"] == "finish"
    assert json.loads(rust.stdout)["busy"]["pid"] == os.getpid()

    # A journal from before #1121 has no owner: read, and classified exactly
    # as before, by the slot's dead owner.
    del journal["owner"]
    wrkslots_cli._write_journal(config, journal, journal_path=path)
    (rc, _out, err), rust = _classify_both(root)
    assert rc == 3 and "interrupted" in err, err
    assert rust.returncode not in (0, 75) and "interrupted" in rust.stderr, rust.stderr


def _finish_journals(root: Path) -> list[dict[str, object]]:
    """Every finish journal a removal recorded in its event log, in order."""

    journals: list[dict[str, object]] = []
    for event_path in sorted((root / "worktrees" / "EVENTS.node-a").iterdir()):
        event: dict[str, object] = json.loads(event_path.read_text())
        payload = cast(dict[str, object], event["payload"])
        if event["kind"] == "operation-progress-recorded":
            journal = cast(dict[str, object], payload["journal"])
            if journal.get("kind") == "finish":
                journals.append(journal)
    return journals


def _scrub_finish_journal(journal: dict[str, object]) -> str:
    """The journal as written, minus what differs per process and per run."""

    copy: dict[str, object] = json.loads(json.dumps(journal))
    copy["finished_at"] = "T"
    copy["archive_id"] = "ID"
    copy["fenced"] = re.sub(r"[0-9a-f]{32}$", "UUID", str(copy["fenced"]))
    owner = cast(dict[str, object], copy["owner"])
    owner["pid"] = owner["start_ticks"] = "REMOVER"
    record = cast(dict[str, object], copy["record"])
    _scrub_doc({"slots": [record]})
    # Each fixture root records its own dead slot owner.
    for key in ("owner", "coordinator_lease"):
        identity = record.get(key)
        if isinstance(identity, dict):
            identity["pid"] = identity["start_ticks"] = "SLOT-OWNER"
    for item in cast(list[dict[str, object]], copy.get("salvage", [])):
        item["source_head"] = item["salvage_commit"] = "HEAD"
    return json.dumps(copy, indent=2, sort_keys=True)


def test_remove_writes_the_same_finish_journal_as_python(tmp_path: Path) -> None:
    """#1121: both editions write equivalent finish journals, owner included."""

    with _own_fs(tmp_path) as base:
        rust_root, oracle_root = _pair(base)
        assert _python_remove(oracle_root) == 0
        result = _rust_remove(rust_root)
        assert result.returncode == 0, result.stderr
        rust_journals = _finish_journals(rust_root)
        oracle_journals = _finish_journals(oracle_root)
        assert rust_journals and len(rust_journals) == len(oracle_journals)
        for rust_journal, oracle_journal in zip(rust_journals, oracle_journals):
            left, right = _scrub_finish_journal(rust_journal), _scrub_finish_journal(oracle_journal)
            assert left == right, "\n".join(difflib.unified_diff(right.splitlines(), left.splitlines(), "python", "rust", lineterm=""))
        # Each names its own remover: this process in Python, the binary in Rust.
        assert cast(dict[str, object], oracle_journals[0]["owner"])["pid"] == os.getpid()
        assert cast(dict[str, object], rust_journals[0]["owner"])["pid"] != os.getpid()


def test_recovery_rewrites_the_finish_journal_owner(tmp_path: Path) -> None:
    """#1121: a resumed finish journal names the resuming process."""

    with _own_fs(tmp_path) as base:
        root = _fixture(base / "project")
        _path, journal = _interrupted_python_remove(root)
        remover = cast(dict[str, object], journal["owner"])["pid"]
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            rc = wrkslots_cli.main(
                ["--project-root", str(root), "recover", "--coordinator-pid", str(os.getpid())]
            )
        assert rc == 0
        owners = [cast(dict[str, object], item["owner"])["pid"] for item in _finish_journals(root)]
        assert owners[0] == remover
        assert owners[-1] == os.getpid()
        assert not (root / "worktrees" / "ACTIVE.node-a.journal").exists()


def test_an_older_reader_refuses_only_an_outstanding_owner_journal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """#1121's upgrade exposure, measured with a stand-in for an older reader.

    A reader from before #1121 checks finish-journal keys exactly and does not
    know ``owner``.  While a finish journal carrying it is outstanding, its
    audit and recover refuse ("unknown owner"), so only a newer pin can recover
    that journal after a crash; status and doctor are unaffected, and
    mutations refuse as they already do while any journal is outstanding.  Once
    the removal completed, nothing changes for it: the event log keeps the
    journal, but only an outstanding one is checked.
    """

    with _own_fs(tmp_path) as base:
        outstanding = _fixture(base / "outstanding")
        _interrupted_python_remove(outstanding)
        completed = _fixture(base / "completed")
        assert _python_remove(completed) == 0
        monkeypatch.setattr(
            wrkslots_cli, "_FINISH_JOURNAL_OPTIONAL",
            wrkslots_cli._FINISH_JOURNAL_OPTIONAL - {"owner"},
        )

        def run(root: Path, *argv: str) -> tuple[int, str]:
            err = io.StringIO()
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
                rc = wrkslots_cli.main(["--project-root", str(root), *argv])
            return rc, err.getvalue()

        pid = str(os.getpid())
        assert run(outstanding, "status") == (0, "")
        assert run(outstanding, "doctor") == (0, "")
        for argv in (("audit",), ("recover", "--coordinator-pid", pid)):
            rc, err = run(outstanding, *argv)
            assert rc == 3 and "finish journal has invalid fields: unknown owner" in err, (argv, err)
        assert run(completed, "status") == (0, "")
        assert run(completed, "audit")[0] == 0
