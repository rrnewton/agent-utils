"""Differential: wrkslotsd finish against the Python finish mutation.

#562 rust-primary-wrkslots slice 35, the tenth writer slice. The
Python oracle is `_cmd_finish` on a copy of the fixture registry;
the Rust writer runs on the original. Resulting ACTIVE records and
`handoff-recorded` events must match apart from the wall-clock
stamps and each fixture's own Git history.
Temporary fixture registries only: writers are never pointed at a
live registry before the edition cutover.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
from pathlib import Path
from typing import cast

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.rust_binary import rust_wrkslotsd


AU_ROOT = Path(__file__).resolve().parents[3]
VALIDATION = ["cargo test: 88 passed"]


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


def _live_identity() -> dict[str, object]:
    identity = wrkslots_cli._read_process_identity(os.getpid())
    return {
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    }


def _fixture(base: Path, *, cache_file: bool = False) -> Path:
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
    if cache_file:
        (upstream / "cache").mkdir()
        (upstream / "cache" / "x").write_text("cached\n")
        _git(upstream, "add", "cache/x")
        _git(upstream, "commit", "-q", "-m", "cache")
        head = _git(upstream, "rev-parse", "HEAD")
    _git(upstream, "remote", "add", "origin", str(upstream))
    _git(upstream, "fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*")
    _git(
        upstream, "worktree", "add", "-q", "-b", "suite/work",
        str(root / "worktrees" / "slot-a" / "repo"), "HEAD",
    )
    identity = _live_identity()
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


def _rust_finish(
    root: Path,
    *,
    validation: list[str] | None = None,
    generation: int = 1,
    fixture: bool = True,
    discard: bool = False,
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    args = [
        str(rust_wrkslotsd()), "finish", "slot-a", "--agent", "agent-slot-a",
        "--owner-pid", str(os.getpid()), "--expected-generation", str(generation),
        "--root", str(root),
    ]
    for entry in VALIDATION if validation is None else validation:
        args.extend(["--validation", entry])
    if discard:
        args.append("--discard-unaccounted")
    return subprocess.run(
        args, cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env
    )


def _python_finish(
    root: Path,
    *,
    validation: list[str] | None = None,
    generation: int = 1,
    discard: bool = False,
) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot="slot-a", agent="agent-slot-a",
        owner_pid=os.getpid(), expected_generation=generation,
        validation=VALIDATION if validation is None else validation,
        discard_unaccounted=discard,
        limitation=[], wait_lock=0.0,
    )
    return wrkslots_cli._cmd_finish(args)


def _active_doc(root: Path) -> dict[str, object]:
    doc: dict[str, object] = json.loads((root / "worktrees" / "ACTIVE.node-a.json").read_text())
    for record in cast(list[dict[str, object]], doc["slots"]):
        handoff = record.get("handoff")
        if isinstance(handoff, dict):
            handoff.pop("recorded_at")
        for checkout in cast(list[dict[str, object]], record["checkouts"]):
            checkout["head"] = "HEAD"
            checkout["start_point"] = "HEAD"
            checkout["remote_url_sha256"] = "URLSHA"
    return doc


def _events(root: Path) -> list[dict[str, object]]:
    directory = root / "worktrees" / "EVENTS.node-a"
    if not directory.is_dir():
        return []
    events: list[dict[str, object]] = []
    for path in sorted(directory.iterdir()):
        event: dict[str, object] = json.loads(path.read_text())
        assert event["sha256"] == wrkslots_cli._event_digest(
            {k: v for k, v in event.items() if k != "sha256"}
        )
        event.pop("recorded_at")
        event.pop("sha256")
        event.pop("previous_sha256")
        payload = cast(dict[str, object], event["payload"])
        payload.pop("previous_record_sha256", None)
        record = payload.get("record")
        if isinstance(record, dict):
            handoff = record.get("handoff")
            if isinstance(handoff, dict):
                handoff.pop("recorded_at")
            for checkout in cast(list[dict[str, object]], record["checkouts"]):
                checkout["head"] = "HEAD"
                checkout["start_point"] = "HEAD"
                checkout["remote_url_sha256"] = "URLSHA"
        events.append(event)
    return events


def test_finish_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    assert _python_finish(oracle_root) == 0
    result = _rust_finish(rust_root)
    assert result.returncode == 0, result.stderr
    outcome = json.loads(result.stdout)
    assert outcome["outcome"] == "handoff recorded"
    assert outcome["continuation"] == (
        "wrkslots remove slot-a --coordinator-pid "
        "<current-coordinator-pid> --expected-generation 1"
    )
    assert _active_doc(rust_root) == _active_doc(oracle_root)
    assert [event["kind"] for event in _events(rust_root)] == [
        event["kind"] for event in _events(oracle_root)
    ]
    assert _events(rust_root)[-1]["payload"] == _events(oracle_root)[-1]["payload"]


def test_finish_dirty_checkout_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    for root in (rust_root, oracle_root):
        (root / "worktrees" / "slot-a" / "repo" / "notes.txt").write_text("wip\n")
    with pytest.raises(Exception) as caught:
        _python_finish(oracle_root)
    assert "is dirty or has untracked/ignored files" in str(caught.value)
    result = _rust_finish(rust_root)
    assert result.returncode == 1
    assert "is dirty or has untracked/ignored files" in result.stderr


def test_finish_unlanded_commit_refuses(tmp_path: Path) -> None:
    (rust_root, _rust_up, rust_co), (oracle_root, _oracle_up, oracle_co) = _bare_pair(tmp_path)
    for checkout in (rust_co, oracle_co):
        (checkout / "file.txt").write_text("two\n")
        _git(checkout, "add", "file.txt")
        _git(checkout, "commit", "-q", "-m", "two")
    with pytest.raises(Exception) as caught:
        _python_finish(oracle_root)
    assert "has unaccounted commits" in str(caught.value)
    result = _rust_finish(rust_root)
    assert result.returncode == 1
    assert "has unaccounted commits" in result.stderr


def test_finish_blank_validation_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    with pytest.raises(Exception) as caught:
        _python_finish(oracle_root, validation=[" "])
    assert "at least one non-empty --validation entry" in str(caught.value)
    result = _rust_finish(rust_root, validation=[" "])
    assert result.returncode == 1
    assert "at least one non-empty --validation entry" in result.stderr


def test_finish_twice_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    assert _python_finish(oracle_root) == 0
    assert _rust_finish(rust_root).returncode == 0
    with pytest.raises(Exception) as caught:
        _python_finish(oracle_root)
    assert "already has a recorded handoff" in str(caught.value)
    result = _rust_finish(rust_root)
    assert result.returncode == 1
    assert "already has a recorded handoff" in result.stderr


def test_finish_tracked_cache_path_refuses(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust", cache_file=True)
    oracle_root = _fixture(tmp_path / "oracle", cache_file=True)
    for root in (rust_root, oracle_root):
        config = root / ".wrkslots.yml"
        config.write_text(config.read_text() + "cache_globs:\n  - cache/[^x]\n")
    with pytest.raises(Exception) as caught:
        _python_finish(oracle_root)
    assert "overlaps tracked source" in str(caught.value)
    result = _rust_finish(rust_root)
    assert result.returncode == 1
    assert "overlaps tracked source" in result.stderr


def test_finish_detached_checkout_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    for root in (rust_root, oracle_root):
        _git(root / "worktrees" / "slot-a" / "repo", "checkout", "-q", "--detach", "HEAD")
    with pytest.raises(Exception) as caught:
        _python_finish(oracle_root)
    assert "not detached" in str(caught.value)
    result = _rust_finish(rust_root)
    assert result.returncode == 1
    assert "not detached" in result.stderr


def test_finish_extra_slot_entry_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    for root in (rust_root, oracle_root):
        (root / "worktrees" / "slot-a" / "stray").mkdir()
    with pytest.raises(Exception) as caught:
        _python_finish(oracle_root)
    assert "does not match its record" in str(caught.value)
    result = _rust_finish(rust_root)
    assert result.returncode == 1
    # The Rust storage reader names the same entry with its own text.
    assert "which its record does not name" in result.stderr


def test_finish_stale_generation_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    with pytest.raises(Exception) as caught:
        _python_finish(oracle_root, generation=2)
    assert "stale generation for slot slot-a: expected 2, current 1" in str(caught.value)
    result = _rust_finish(rust_root, generation=2)
    assert result.returncode == 1
    assert "stale generation for slot slot-a: expected 2, current 1" in result.stderr


def test_finish_refuses_without_fixture_attestation(tmp_path: Path) -> None:
    rust_root, _oracle_root = _pair(tmp_path)
    result = _rust_finish(rust_root, fixture=False)
    assert result.returncode == 1
    assert "fixture-only" in result.stderr
    doc: dict[str, object] = json.loads(
        (rust_root / "worktrees" / "ACTIVE.node-a.json").read_text()
    )
    record = cast(list[dict[str, object]], doc["slots"])[0]
    assert record["handoff"] is None


def _fixture_bare(base: Path) -> tuple[Path, Path, Path]:
    """Fixture whose origin is a bare repo separate from the source."""
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
    bare = base / "origin.git"
    _git(base, "clone", "-q", "--bare", str(upstream), str(bare))
    _git(upstream, "remote", "add", "origin", str(bare))
    _git(upstream, "fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*")
    checkout = root / "worktrees" / "slot-a" / "repo"
    _git(upstream, "worktree", "add", "-q", "-b", "suite/work", str(checkout), "HEAD")
    identity = _live_identity()
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
                "remote_url_sha256": hashlib.sha256(str(bare).encode()).hexdigest(),
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
    return root, upstream, checkout


def _bare_pair(tmp_path: Path) -> tuple[tuple[Path, Path, Path], tuple[Path, Path, Path]]:
    return _fixture_bare(tmp_path / "rust"), _fixture_bare(tmp_path / "oracle")


def _commit_in(checkout: Path, message: str) -> str:
    (checkout / "a.txt").write_text(f"{message}\n")
    _git(checkout, "add", "a.txt")
    _git(checkout, "commit", "-q", "-m", message)
    return _git(checkout, "rev-parse", "HEAD")


def _accounting_of(outcome: dict[str, object]) -> dict[str, object]:
    return cast(dict[str, object], outcome["accounting"])


def test_finish_accounting_landed_exact_both_editions(tmp_path: Path) -> None:
    (rust_root, rust_up, rust_co), (oracle_root, oracle_up, oracle_co) = _bare_pair(tmp_path)
    for upstream, checkout in ((rust_up, rust_co), (oracle_up, oracle_co)):
        commit = _commit_in(checkout, "landed exact")
        _git(upstream, "merge", "-q", "--ff-only", commit)
        _git(upstream, "push", "-q", "origin", "main")
    assert _python_finish(oracle_root) == 0
    result = _rust_finish(rust_root)
    assert result.returncode == 0, result.stderr
    assert _accounting_of(json.loads(result.stdout))["landed"] == 1


def test_finish_accounting_landed_replay_both_editions(tmp_path: Path) -> None:
    (rust_root, rust_up, rust_co), (oracle_root, oracle_up, oracle_co) = _bare_pair(tmp_path)
    for upstream, checkout in ((rust_up, rust_co), (oracle_up, oracle_co)):
        commit = _commit_in(checkout, "landed replay")
        _git(upstream, "cherry-pick", commit)
        _git(upstream, "commit", "-q", "--amend", "-m", "landed replay (replayed)")
        _git(upstream, "push", "-q", "origin", "main")
    assert _python_finish(oracle_root) == 0
    result = _rust_finish(rust_root)
    assert result.returncode == 0, result.stderr
    assert _accounting_of(json.loads(result.stdout))["landed"] == 1


def test_finish_accounting_open_both_editions(tmp_path: Path) -> None:
    (rust_root, _rust_up, rust_co), (oracle_root, _oracle_up, oracle_co) = _bare_pair(tmp_path)
    for checkout in (rust_co, oracle_co):
        _commit_in(checkout, "open change")
        _git(checkout, "push", "-q", "origin", "HEAD:refs/heads/pr/suite-work")
    assert _python_finish(oracle_root) == 0
    result = _rust_finish(rust_root)
    assert result.returncode == 0, result.stderr
    assert _accounting_of(json.loads(result.stdout))["open"] == 1


def test_finish_accounting_open_replay_both_editions(tmp_path: Path) -> None:
    (rust_root, rust_up, rust_co), (oracle_root, oracle_up, oracle_co) = _bare_pair(tmp_path)
    for upstream, checkout in ((rust_up, rust_co), (oracle_up, oracle_co)):
        commit = _commit_in(checkout, "open replay")
        _git(upstream, "cherry-pick", commit)
        _git(upstream, "commit", "-q", "--amend", "-m", "open replay (replayed)")
        _git(upstream, "push", "-q", "origin", "HEAD:refs/heads/pr/open-replay")
        _git(upstream, "reset", "-q", "--hard", "HEAD~1")
    assert _python_finish(oracle_root) == 0
    result = _rust_finish(rust_root)
    assert result.returncode == 0, result.stderr
    assert _accounting_of(json.loads(result.stdout))["open"] == 1


def test_finish_accounting_many_remote_refs_both_editions(tmp_path: Path) -> None:
    (rust_root, rust_up, rust_co), (oracle_root, oracle_up, oracle_co) = _bare_pair(tmp_path)
    for upstream, checkout in ((rust_up, rust_co), (oracle_up, oracle_co)):
        commit = _commit_in(checkout, "open replay many")
        _git(upstream, "cherry-pick", commit)
        _git(upstream, "commit", "-q", "--amend", "-m", "open replay many (replayed)")
        _git(upstream, "push", "-q", "origin", "HEAD:refs/heads/pr/open-replay")
        _git(upstream, "reset", "-q", "--hard", "HEAD~1")
        base = _git(upstream, "rev-parse", "HEAD")
        for index in range(300):
            _git(upstream, "update-ref", f"refs/heads/rescue/r{index:03d}", base)
        _git(upstream, "push", "-q", "origin", "+refs/heads/rescue/*:refs/heads/rescue/*")
    assert _python_finish(oracle_root) == 0
    result = _rust_finish(rust_root)
    assert result.returncode == 0, result.stderr
    assert _accounting_of(json.loads(result.stdout))["open"] == 1


def test_finish_accounting_unaccounted_both_editions(tmp_path: Path) -> None:
    (rust_root, _rust_up, rust_co), (oracle_root, _oracle_up, oracle_co) = _bare_pair(tmp_path)
    for checkout in (rust_co, oracle_co):
        _commit_in(checkout, "secret wip")
    with pytest.raises(Exception) as caught:
        _python_finish(oracle_root)
    assert "unaccounted=1" in str(caught.value)
    result = _rust_finish(rust_root)
    assert result.returncode == 1
    assert "unaccounted=1" in result.stderr


def _add_provisioned_symlink(root: Path) -> None:
    checkout = root / "worktrees" / "slot-a" / "repo"
    (checkout / "provisioned-link").symlink_to("file.txt")
    entry = wrkslots_cli._describe_provisioned(checkout, "provisioned-link")
    active = root / "worktrees" / "ACTIVE.node-a.json"
    doc: dict[str, object] = json.loads(active.read_text())
    record = cast(list[dict[str, object]], doc["slots"])[0]
    cast(list[dict[str, object]], record["checkouts"])[0]["provisioned"] = [
        wrkslots_cli._provisioned_to_obj(entry)
    ]
    active.write_text(json.dumps(doc))


def test_finish_provisioned_symlink_both_editions(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    for root in (rust_root, oracle_root):
        _add_provisioned_symlink(root)
    assert _python_finish(oracle_root) == 0
    result = _rust_finish(rust_root)
    assert result.returncode == 0, result.stderr


def test_finish_provisioned_changed_both_editions(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    for root in (rust_root, oracle_root):
        _add_provisioned_symlink(root)
        link = root / "worktrees" / "slot-a" / "repo" / "provisioned-link"
        link.unlink()
        link.symlink_to("other.txt")
    with pytest.raises(Exception) as caught:
        _python_finish(oracle_root)
    assert "provisioned path changed" in str(caught.value)
    result = _rust_finish(rust_root)
    assert result.returncode == 1
    assert "provisioned path changed" in result.stderr


def test_finish_accounting_discard_both_editions(tmp_path: Path) -> None:
    (rust_root, _rust_up, rust_co), (oracle_root, _oracle_up, oracle_co) = _bare_pair(tmp_path)
    commits = {}
    for label, checkout in (("rust", rust_co), ("oracle", oracle_co)):
        commits[label] = _commit_in(checkout, "throwaway")
    assert _python_finish(oracle_root, discard=True) == 0
    result = _rust_finish(rust_root, discard=True)
    assert result.returncode == 0, result.stderr
    for root, label in ((rust_root, "rust"), (oracle_root, "oracle")):
        doc: dict[str, object] = json.loads(
            (root / "worktrees" / "ACTIVE.node-a.json").read_text()
        )
        handoff = cast(
            dict[str, object],
            cast(list[dict[str, object]], doc["slots"])[0]["handoff"],
        )
        assert handoff["discarded_unaccounted"] == [commits[label]]
        assert list((root / "worktrees" / "wrkslots-salvage").glob("*.bundle"))
