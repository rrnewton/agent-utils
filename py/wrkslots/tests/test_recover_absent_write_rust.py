"""Differential: wrkslotsd recover-absent-agent-row(s) against Python.

#562 rust-primary-wrkslots slice 39, the fourteenth writer slice.
The Python oracle is `_cmd_recover_absent_agent_row` /
`_cmd_recover_absent_agent_rows` on a copy of the fixture registry;
the Rust writer runs on the original. The Rust slice covers the
non-destructive outcomes (already-recovered, journal would-resume,
storage/digest refusals); the census-and-salvage proof path stays
with the Python edition and refuses explicitly. Temporary fixture
registries only: writers are never pointed at a live registry
before the edition cutover.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import socket
import subprocess
from pathlib import Path
from typing import cast

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
        "agent": "agent-slot-a",
        "task": "do work",
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
                "path": "worktrees/slot-a/repo",
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
        str(root / "worktrees" / "slot-a" / "repo"), "HEAD",
    )
    record = _record(head, upstream)
    (root / "worktrees" / f"ACTIVE.{MACHINE}.json").write_text(
        json.dumps({"schema": 2, "machine": MACHINE, "revision": 7, "slots": [record]})
    )
    (root / "worktrees" / f"ARCHIVED.{MACHINE}.json").write_text(
        json.dumps({"schema": 2, "machine": MACHINE, "revision": 3, "records": []})
    )
    return root, record


def _record_sha(root: Path, record: dict[str, object]) -> str:
    config = wrkslots_cli._load_config(str(root), None)
    state = wrkslots_cli._load_active(config)
    assert state.slots, "fixture row missing"
    return wrkslots_cli._record_sha256(state.slots[0])


def _mark_recovered(root: Path, record: dict[str, object], sha: str) -> None:
    """Make the row an exact prior recovery on both state files."""
    config = wrkslots_cli._load_config(str(root), None)
    record_obj = wrkslots_cli._record_from_obj(record, "fixture record")
    item = wrkslots_cli.AbsentAgentRow(MACHINE, "slot-a", 1, sha)
    entry = wrkslots_cli._absent_agent_archive_entry(
        record_obj, item, FINISHED, (), ()
    )
    active_path = root / "worktrees" / f"ACTIVE.{MACHINE}.json"
    active_path.write_text(
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
            "action": "absent-agent-row-recovered",
            "slot": "slot-a",
            "previous_revision": 3,
            "revision": 4,
            "record": entry,
            "evidence": {
                "archive_id": entry["archive_id"],
                "source_record_sha256": sha,
                "physical_storage": "externally-absent",
            },
        },
    )
    archive_path.write_text(
        json.dumps(
            {"schema": 2, "machine": MACHINE, "revision": 4, "records": [entry]}
        )
    )


def _rust_row(
    root: Path, sha: str, *, slot: str = "slot-a", fixture: bool = True
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    return subprocess.run(
        [
            "cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
            "-p", "wrkslotsd", "--", "recover-absent-agent-row", slot,
            "--expected-generation", "1", "--record-sha256", sha,
            "--root", str(root),
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _python_row(root: Path, sha: str, *, slot: str = "slot-a") -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot=slot, expected_generation=1,
        record_sha256=sha, apply=False, coordinator_authorized=False,
        coordinator_pid=None, wait_lock=0.0,
    )
    return wrkslots_cli._cmd_recover_absent_agent_row(args)


def _state_texts(root: Path) -> tuple[str, str]:
    return (
        (root / "worktrees" / f"ACTIVE.{MACHINE}.json").read_text(),
        (root / "worktrees" / f"ARCHIVED.{MACHINE}.json").read_text(),
    )


def test_absent_row_already_recovered_matches_python(tmp_path: Path) -> None:
    rust_root, record = _fixture(tmp_path / "rust")
    oracle_root, oracle_record = _fixture(tmp_path / "oracle")
    assert record.keys() == oracle_record.keys()
    rust_sha = _record_sha(rust_root, record)
    oracle_sha = _record_sha(oracle_root, oracle_record)
    _mark_recovered(rust_root, record, rust_sha)
    _mark_recovered(oracle_root, oracle_record, oracle_sha)
    before = _state_texts(rust_root)
    assert _python_row(oracle_root, oracle_sha) == 0
    result = _rust_row(rust_root, rust_sha)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["outcome"] == "already-recovered"
    assert _state_texts(rust_root) == before


def test_absent_row_storage_present_refuses(tmp_path: Path) -> None:
    rust_root, record = _fixture(tmp_path / "rust")
    oracle_root, oracle_record = _fixture(tmp_path / "oracle")
    rust_sha = _record_sha(rust_root, record)
    oracle_sha = _record_sha(oracle_root, oracle_record)
    with pytest.raises(Exception) as caught:
        _python_row(oracle_root, oracle_sha)
    assert "still has physical storage" in str(caught.value)
    result = _rust_row(rust_root, rust_sha)
    assert result.returncode == 1
    assert "still has physical storage" in result.stderr


def test_absent_row_digest_mismatch_refuses(tmp_path: Path) -> None:
    rust_root, record = _fixture(tmp_path / "rust")
    oracle_root, _oracle_record = _fixture(tmp_path / "oracle")
    with pytest.raises(Exception) as caught:
        _python_row(oracle_root, EMPTY_SHA)
    assert "changed: expected" in str(caught.value)
    result = _rust_row(rust_root, EMPTY_SHA)
    assert result.returncode == 1
    assert "changed: expected" in result.stderr


def test_absent_row_neither_active_nor_recovered_refuses(tmp_path: Path) -> None:
    rust_root, record = _fixture(tmp_path / "rust")
    oracle_root, _oracle_record = _fixture(tmp_path / "oracle")
    for root in (rust_root, oracle_root):
        (root / "worktrees" / f"ACTIVE.{MACHINE}.json").write_text(
            json.dumps({"schema": 2, "machine": MACHINE, "revision": 7, "slots": []})
        )
    with pytest.raises(Exception) as caught:
        _python_row(oracle_root, EMPTY_SHA)
    assert "is neither active nor an exact prior recovery" in str(caught.value)
    result = _rust_row(rust_root, EMPTY_SHA)
    assert result.returncode == 1
    assert "is neither active nor an exact prior recovery" in result.stderr


def _write_journal(root: Path, record: dict[str, object], sha: str) -> None:
    record_obj = wrkslots_cli._record_from_obj(record, "fixture record")
    item = wrkslots_cli.AbsentAgentRow(MACHINE, "slot-a", 1, sha)
    witnesses = tuple(
        wrkslots_cli._AbsentAgentBranchWitness(
            checkout=checkout.name,
            branch_ref=f"refs/heads/{checkout.branch}",
            state="present",
            head=checkout.head,
        )
        for checkout in record_obj.checkouts
    )
    receipts = wrkslots_cli._absent_agent_planned_receipts(
        record_obj,
        witnesses,
        include_branch_witness=False,
        rescue_root=wrkslots_cli.ABSENT_AGENT_RESCUE_REF_ROOT,
        containment=None,
    )
    entry = wrkslots_cli._absent_agent_archive_entry(
        record_obj, item, FINISHED, receipts, ()
    )
    journal = {
        "schema": 2,
        "kind": "recover-absent-agent-row",
        "machine": MACHINE,
        "host_id": "fixture-host",
        "slot": "slot-a",
        "phase": "prepared",
        "actor": _live_identity(),
        "input": {
            "machine": MACHINE,
            "slot": "slot-a",
            "generation": 1,
            "record_sha256": sha,
        },
        "record": record,
        "preserved": [],
        "removed": [],
        "archive_entry": entry,
    }
    (root / "worktrees" / f"ACTIVE.{MACHINE}.journal").write_text(
        json.dumps(journal)
    )


def test_absent_row_journal_would_resume(tmp_path: Path) -> None:
    rust_root, record = _fixture(tmp_path / "rust")
    oracle_root, oracle_record = _fixture(tmp_path / "oracle")
    sha = _record_sha(rust_root, record)
    oracle_sha = _record_sha(oracle_root, oracle_record)
    _write_journal(rust_root, record, sha)
    _write_journal(oracle_root, oracle_record, oracle_sha)
    assert _python_row(oracle_root, oracle_sha) == 0
    result = _rust_row(rust_root, sha)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["outcome"] == "would-resume"


def test_absent_row_minimal_journal_refuses(tmp_path: Path) -> None:
    rust_root, record = _fixture(tmp_path / "rust")
    sha = _record_sha(rust_root, record)
    journal = {
        "kind": "recover-absent-agent-row",
        "input": {
            "machine": MACHINE,
            "slot": "slot-a",
            "generation": 1,
            "record_sha256": sha,
        },
    }
    (rust_root / "worktrees" / f"ACTIVE.{MACHINE}.journal").write_text(
        json.dumps(journal)
    )
    result = _rust_row(rust_root, sha)
    assert result.returncode == 1
    assert "journal is invalid" in result.stderr


def test_absent_row_mismatched_recovery_event_refuses(tmp_path: Path) -> None:
    rust_root, record = _fixture(tmp_path / "rust")
    oracle_root, oracle_record = _fixture(tmp_path / "oracle")
    rust_sha = _record_sha(rust_root, record)
    oracle_sha = _record_sha(oracle_root, oracle_record)
    _mark_recovered(rust_root, record, rust_sha)
    _mark_recovered(oracle_root, oracle_record, oracle_sha)
    for root in (rust_root, oracle_root):
        events = root / "worktrees" / f"EVENTS.{MACHINE}"
        for path in events.iterdir():
            event = json.loads(path.read_text())
            if event["kind"] == "archive-state-recorded":
                event["payload"]["evidence"]["archive_id"] = "bogus:archive"
                event.pop("sha256")
                event["sha256"] = wrkslots_cli._event_digest(event)
                path.write_text(json.dumps(event))
    with pytest.raises(Exception):
        _python_row(oracle_root, oracle_sha)
    result = _rust_row(rust_root, rust_sha)
    assert result.returncode == 1


def test_absent_rows_batch_matches_python(tmp_path: Path) -> None:
    rust_root, record = _fixture(tmp_path / "rust")
    oracle_root, oracle_record = _fixture(tmp_path / "oracle")
    rust_sha = _record_sha(rust_root, record)
    oracle_sha = _record_sha(oracle_root, oracle_record)
    _mark_recovered(rust_root, record, rust_sha)
    _mark_recovered(oracle_root, oracle_record, oracle_sha)

    def python_payload(root: Path, sha: str) -> dict[str, object]:
        args = argparse.Namespace(
            project_root=str(root), machine=None,
            rows=[f"slot-a=1={sha}", f"slot-b=1={EMPTY_SHA}"], input=None,
            apply=False, coordinator_authorized=False, coordinator_pid=None,
            format="json", wait_lock=0.0,
        )
        import io
        from contextlib import redirect_stdout

        buffer = io.StringIO()
        with redirect_stdout(buffer):
            assert wrkslots_cli._cmd_recover_absent_agent_rows(args) == 1
        payload: dict[str, object] = json.loads(buffer.getvalue())
        return payload

    def rust_payload(root: Path, sha: str) -> dict[str, object]:
        env = dict(os.environ)
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
        result = subprocess.run(
            [
                "cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
                "-p", "wrkslotsd", "--", "recover-absent-agent-rows",
                "--row", f"slot-a=1={sha}", "--row", f"slot-b=1={EMPTY_SHA}",
                "--root", str(root),
            ],
            cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
        )
        assert result.returncode == 1, result.stderr
        payload: dict[str, object] = json.loads(result.stdout)
        return payload

    def scrub(payload: dict[str, object]) -> dict[str, object]:
        payload["seconds"] = 0
        for row in cast(list[dict[str, object]], payload["rows"]):
            row["seconds"] = 0
            row.pop("reason", None)
        return payload

    assert scrub(rust_payload(rust_root, rust_sha)) == scrub(
        python_payload(oracle_root, oracle_sha)
    )


def test_absent_row_refuses_without_fixture_attestation(tmp_path: Path) -> None:
    rust_root, record = _fixture(tmp_path / "rust")
    sha = _record_sha(rust_root, record)
    result = _rust_row(rust_root, sha, fixture=False)
    assert result.returncode == 1
    assert "fixture-only" in result.stderr
