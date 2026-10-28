"""Differential: wrkslotsd recover-absent-validate-rows against Python.

#562 rust-primary-wrkslots slice 40, the fifteenth writer slice.
The Python oracle is `_cmd_recover_absent_validate_rows` on a copy
of the fixture registry; the Rust writer runs on the original. The
Rust slice covers the input contract and non-destructive outcomes;
the validation census plan/apply path stays with the Python
edition and refuses explicitly. Temporary fixture registries only:
writers are never pointed at a live registry before the edition
cutover.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import socket
import subprocess
from contextlib import redirect_stdout
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli


AU_ROOT = Path(__file__).resolve().parents[3]
MACHINE = socket.gethostname().split(".", 1)[0]
FINISHED = "2026-10-26T02:00:00Z"
EMPTY_SHA = "e" * 64


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


def _record(head: str, upstream: Path) -> dict[str, object]:
    return {
        "slot": "slot-a",
        "slot_type": "validate",
        "agent": "validate-slot-a",
        "task": "check things",
        "purpose": "testing",
        "machine": MACHINE,
        "generation": 1,
        "created_at": "2026-10-26T00:00:00Z",
        "heartbeat_at": "2026-10-26T01:00:00Z",
        "heartbeat_ttl_seconds": 60,
        "owner": _live_identity(),
        "coordinator_lease": _live_identity(),
        "coordinator_recovery_note": None,
        "handoff": None,
        "checkouts": [
            {
                "name": "repo",
                "path": "worktrees/validate/slot-a/repo",
                "repository": "../upstream-repo",
                "branch": "suite/work",
                "start_point": head,
                "remote": "origin",
                "remote_url_sha256": hashlib.sha256(str(upstream).encode()).hexdigest(),
                "landed_ref": "refs/remotes/origin/main",
                "head": head,
                "containing_remote_refs": ["refs/remotes/origin/main"],
                "vcs": "git",
            }
        ],
    }


def _fixture(base: Path) -> tuple[Path, dict[str, object]]:
    root = base / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        f"machine: {MACHINE}\n"
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
        str(root / "worktrees" / "validate" / "slot-a" / "repo"), "HEAD",
    )
    record = _record(head, upstream)
    (root / "worktrees" / f"ACTIVE.{MACHINE}.json").write_text(
        json.dumps({"schema": 2, "machine": MACHINE, "revision": 7, "slots": [record]})
    )
    (root / "worktrees" / f"ARCHIVED.{MACHINE}.json").write_text(
        json.dumps({"schema": 2, "machine": MACHINE, "revision": 3, "records": []})
    )
    return root, record


def _record_sha(root: Path) -> str:
    config = wrkslots_cli._load_config(str(root), None)
    state = wrkslots_cli._load_active(config)
    assert state.slots, "fixture row missing"
    return wrkslots_cli._record_sha256(state.slots[0])


def _mark_recovered(root: Path, record: dict[str, object], sha: str) -> None:
    config = wrkslots_cli._load_config(str(root), None)
    record_obj = wrkslots_cli._record_from_obj(record, "fixture record")
    item = wrkslots_cli.AbsentValidateRow(MACHINE, "slot-a", 1, sha)
    entry = wrkslots_cli._absent_validate_archive_entry(record_obj, item, FINISHED)
    (root / "worktrees" / f"ACTIVE.{MACHINE}.json").write_text(
        json.dumps({"schema": 2, "machine": MACHINE, "revision": 7, "slots": []})
    )
    archive_path = root / "worktrees" / f"ARCHIVED.{MACHINE}.json"
    archive_path.write_text(
        json.dumps({"schema": 2, "machine": MACHINE, "revision": 3, "records": []})
    )
    wrkslots_cli._ensure_event_log(config)
    wrkslots_cli._write_event_file(
        config,
        MACHINE,
        "archive-state-recorded",
        {
            "action": "absent-validation-row-recovered",
            "slot": "slot-a",
            "previous_revision": 3,
            "revision": 4,
            "record": entry,
            "evidence": {
                "archive_id": entry["archive_id"],
                "source_record_sha256": sha,
                "validation_outcome": "unknown",
            },
        },
    )
    archive_path.write_text(
        json.dumps(
            {"schema": 2, "machine": MACHINE, "revision": 4, "records": [entry]}
        )
    )


def _input_file(root: Path, rows: list[dict[str, object]]) -> Path:
    path = root / "rows.json"
    path.write_text(json.dumps({"schema": 1, "rows": rows}))
    return path


def _row(sha: str, *, slot: str = "slot-a") -> dict[str, object]:
    return {"machine": MACHINE, "slot": slot, "generation": 1, "record_sha256": sha}


def _rust(
    root: Path, input_path: Path, *, fixture: bool = True
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    return subprocess.run(
        [
            "cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
            "-p", "wrkslotsd", "--", "recover-absent-validate-rows",
            "--input", str(input_path), "--format", "json", "--root", str(root),
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _python(root: Path, input_path: Path) -> dict[str, object]:
    args = argparse.Namespace(
        project_root=str(root), machine=None, input=str(input_path), apply=False,
        coordinator_authorized=False, coordinator_pid=None, format="json",
        wait_lock=0.0,
    )
    buffer = io.StringIO()
    with redirect_stdout(buffer):
        assert wrkslots_cli._cmd_recover_absent_validate_rows(args) == 0
    payload: dict[str, object] = json.loads(buffer.getvalue())
    return payload


def test_validate_rows_already_recovered_matches_python(tmp_path: Path) -> None:
    rust_root, record = _fixture(tmp_path / "rust")
    oracle_root, oracle_record = _fixture(tmp_path / "oracle")
    assert record.keys() == oracle_record.keys()
    rust_sha = _record_sha(rust_root)
    oracle_sha = _record_sha(oracle_root)
    _mark_recovered(rust_root, record, rust_sha)
    _mark_recovered(oracle_root, oracle_record, oracle_sha)
    rust_input = _input_file(rust_root, [_row(rust_sha)])
    oracle_input = _input_file(oracle_root, [_row(oracle_sha)])
    expected = _python(oracle_root, oracle_input)
    result = _rust(rust_root, rust_input)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == expected


def test_validate_rows_storage_present_refuses(tmp_path: Path) -> None:
    rust_root, _record = _fixture(tmp_path / "rust")
    oracle_root, _oracle_record = _fixture(tmp_path / "oracle")
    rust_sha = _record_sha(rust_root)
    oracle_sha = _record_sha(oracle_root)
    oracle_input = _input_file(oracle_root, [_row(oracle_sha)])
    args = argparse.Namespace(
        project_root=str(oracle_root), machine=None, input=str(oracle_input),
        apply=False, coordinator_authorized=False, coordinator_pid=None,
        format="json", wait_lock=0.0,
    )
    with pytest.raises(Exception) as caught:
        wrkslots_cli._cmd_recover_absent_validate_rows(args)
    assert "still has physical storage" in str(caught.value)
    result = _rust(rust_root, _input_file(rust_root, [_row(rust_sha)]))
    assert result.returncode == 1
    assert "still has physical storage" in result.stderr


def test_validate_rows_neither_refuses(tmp_path: Path) -> None:
    rust_root, _record = _fixture(tmp_path / "rust")
    oracle_root, _oracle_record = _fixture(tmp_path / "oracle")
    for root in (rust_root, oracle_root):
        (root / "worktrees" / f"ACTIVE.{MACHINE}.json").write_text(
            json.dumps({"schema": 2, "machine": MACHINE, "revision": 7, "slots": []})
        )
    oracle_input = _input_file(oracle_root, [_row(EMPTY_SHA)])
    args = argparse.Namespace(
        project_root=str(oracle_root), machine=None, input=str(oracle_input),
        apply=False, coordinator_authorized=False, coordinator_pid=None,
        format="json", wait_lock=0.0,
    )
    with pytest.raises(Exception) as caught:
        wrkslots_cli._cmd_recover_absent_validate_rows(args)
    assert "is neither active nor an exact prior absent-row recovery" in str(caught.value)
    result = _rust(rust_root, _input_file(rust_root, [_row(EMPTY_SHA)]))
    assert result.returncode == 1
    assert "is neither active nor an exact prior absent-row recovery" in result.stderr


def test_validate_rows_duplicate_input_refuses(tmp_path: Path) -> None:
    rust_root, _record = _fixture(tmp_path / "rust")
    oracle_root, _oracle_record = _fixture(tmp_path / "oracle")
    sha = _record_sha(rust_root)
    oracle_input = _input_file(oracle_root, [_row(sha), _row(sha)])
    config = wrkslots_cli._load_config(str(oracle_root), None)
    with pytest.raises(Exception) as caught:
        wrkslots_cli._read_absent_validate_rows_input(config, str(oracle_input))
    assert "repeats" in str(caught.value)
    result = _rust(rust_root, _input_file(rust_root, [_row(sha), _row(sha)]))
    assert result.returncode == 1
    assert "repeats" in result.stderr


def test_validate_rows_journal_input_mismatch_refuses(tmp_path: Path) -> None:
    rust_root, _record = _fixture(tmp_path / "rust")
    sha = _record_sha(rust_root)
    journal = {
        "kind": "recover-absent-validate-rows",
        "input_sha256": "0" * 64,
    }
    (rust_root / "worktrees" / f"ACTIVE.{MACHINE}.journal").write_text(
        json.dumps(journal)
    )
    result = _rust(rust_root, _input_file(rust_root, [_row(sha)]))
    assert result.returncode == 1
    assert "differs from the interrupted batch" in result.stderr


def test_validate_rows_refuses_without_fixture_attestation(tmp_path: Path) -> None:
    rust_root, _record = _fixture(tmp_path / "rust")
    sha = _record_sha(rust_root)
    result = _rust(rust_root, _input_file(rust_root, [_row(sha)]), fixture=False)
    assert result.returncode == 1
    assert "fixture-only" in result.stderr
