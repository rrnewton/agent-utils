"""Differential: wrkslotsd heartbeat against the Python heartbeat mutation.

#562 rust-primary-wrkslots slice 30, the fifth writer slice. The
Python oracle is `_cmd_write_handoff` on a copy of the fixture registry;
the Rust writer runs on the original. Resulting handoff sidecars and publication events must match apart
from the wall-clock stamps (and digests, re-verified with the Python
digest function).
Temporary fixture registries only: writers are never pointed at a
live registry before the edition cutover.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import cast

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.rust_binary import rust_wrkslotsd


AU_ROOT = Path(__file__).resolve().parents[3]
DIGEST = "b" * 64
HOLD_NAME = "HOLD.6.node-a.slot-a.json"


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


def _rust(root: Path, slot: str, reason: str, *, fixture: bool = True) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    return subprocess.run(
        [str(rust_wrkslotsd()), "hold", slot, "--reason", reason, "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _python(root: Path, slot: str, reason: str) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot=slot, reason=reason, wait_lock=0.0
    )
    return wrkslots_cli._cmd_hold(args)


def _pair(tmp_path: Path) -> tuple[Path, Path]:
    return _fixture(tmp_path / "rust"), _fixture(tmp_path / "oracle")


def _hold_record(root: Path) -> dict[str, object] | None:
    path = root / "worktrees" / HOLD_NAME
    if not path.exists():
        return None
    record: dict[str, object] = json.loads(path.read_text())
    record.pop("held_at")
    return record


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
        # Each fixture pair has its own Git history, so commit ids differ.
        payload = event.get("payload")
        if isinstance(payload, dict):
            active = payload.get("active")
            if isinstance(active, dict):
                for record in active.get("slots", []):
                    for checkout in record.get("checkouts", []):
                        checkout["head"] = "HEAD"
                        checkout["start_point"] = "HEAD"
        events.append(event)
    return events




def _rust_hb(
    root: Path,
    slot: str,
    agent: str,
    owner_pid: int,
    generation: int,
    *,
    fixture: bool = True,
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    return subprocess.run(
        [str(rust_wrkslotsd()), "heartbeat", slot, "--agent", agent,
         "--owner-pid", str(owner_pid), "--expected-generation", str(generation),
         "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _python_hb(root: Path, slot: str, agent: str, owner_pid: int, generation: int) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot=slot, agent=agent,
        owner_pid=owner_pid, expected_generation=generation, wait_lock=0.0,
    )
    return wrkslots_cli._cmd_heartbeat(args)


def _active(root: Path) -> dict[str, object]:
    doc: dict[str, object] = json.loads((root / "worktrees" / "ACTIVE.node-a.json").read_text())
    for record in doc["slots"]:  # type: ignore[attr-defined]
        record.pop("heartbeat_at")
    return doc




SIDECAR_NAME = "HANDOFF.6.node-a.6.slot-a.1.json"


def _source(base: Path) -> Path:
    base.mkdir(parents=True, exist_ok=True)
    path = base / "notes.md"
    path.write_text("# Handoff\n\nAll done.\n")
    return path


def _rust_wh(
    root: Path, source: Path, *, fixture: bool = True
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    return subprocess.run(
        [str(rust_wrkslotsd()), "write-handoff", "slot-a", "--agent", "agent-slot-a",
         "--owner-pid", str(os.getpid()), "--expected-generation", "1",
         "--from-file", str(source), "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _python_wh(root: Path, source: Path) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot="slot-a", agent="agent-slot-a",
        owner_pid=os.getpid(), expected_generation=1, from_file=str(source),
        wait_lock=0.0,
    )
    return wrkslots_cli._cmd_write_handoff(args)


def _sidecar(root: Path) -> dict[str, object] | None:
    path = root / "worktrees" / SIDECAR_NAME
    if not path.exists():
        return None
    doc: dict[str, object] = json.loads(path.read_text())
    doc.pop("recorded_at")
    identity = cast(dict[str, object], doc["source_identity"])
    identity.pop("identity")
    return doc


def test_write_handoff_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    rust_source = _source(tmp_path / "rust-src")
    oracle_source = _source(tmp_path / "oracle-src")
    assert _python_wh(oracle_root, oracle_source) == 0
    result = _rust_wh(rust_root, rust_source)
    assert result.returncode == 0, result.stderr
    rust_doc = _sidecar(rust_root)
    oracle_doc = _sidecar(oracle_root)
    assert rust_doc is not None and oracle_doc is not None
    assert rust_doc.pop("source_path") is not None
    oracle_doc.pop("source_path")
    assert rust_doc == oracle_doc
    assert [event["kind"] for event in _events(rust_root)] == [
        "state-imported",
        "handoff-write-intended",
        "handoff-write-completed",
        "handoff-written",
    ]
    assert [event["kind"] for event in _events(oracle_root)] == [
        "state-imported",
        "handoff-write-intended",
        "handoff-write-completed",
        "handoff-written",
    ]


def test_write_handoff_after_finish_matches_python(tmp_path: Path) -> None:
    """#1868: a recorded finish does not block publishing the sidecar once."""

    rust_root, oracle_root = _pair(tmp_path)
    finish_handoff = {
        "recorded_at": "2026-10-26T01:30:00+00:00",
        "validation": ["pytest"],
        "limitations": [],
        "continuation": "wrkslots remove slot-a",
    }
    for root in (rust_root, oracle_root):
        active_path = root / "worktrees" / "ACTIVE.node-a.json"
        doc = json.loads(active_path.read_text())
        doc["slots"][0]["handoff"] = finish_handoff
        active_path.write_text(json.dumps(doc))
    rust_source = _source(tmp_path / "rust-src")
    oracle_source = _source(tmp_path / "oracle-src")
    assert _python_wh(oracle_root, oracle_source) == 0
    result = _rust_wh(rust_root, rust_source)
    assert result.returncode == 0, result.stderr
    rust_doc = _sidecar(rust_root)
    oracle_doc = _sidecar(oracle_root)
    assert rust_doc is not None and oracle_doc is not None
    # The finish record itself is never modified by the late write.
    for root in (rust_root, oracle_root):
        doc = json.loads((root / "worktrees" / "ACTIVE.node-a.json").read_text())
        assert doc["slots"][0]["handoff"] == finish_handoff


def test_write_handoff_repeat_same_bytes_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    rust_source = _source(tmp_path / "rust-src")
    oracle_source = _source(tmp_path / "oracle-src")
    assert _python_wh(oracle_root, oracle_source) == 0
    assert _rust_wh(rust_root, rust_source).returncode == 0
    assert _python_wh(oracle_root, oracle_source) == 0
    result = _rust_wh(rust_root, rust_source)
    assert result.returncode == 0, result.stderr
    assert len(_events(rust_root)) == len(_events(oracle_root))


def test_write_handoff_different_bytes_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    rust_source = _source(tmp_path / "rust-src")
    oracle_source = _source(tmp_path / "oracle-src")
    assert _python_wh(oracle_root, oracle_source) == 0
    assert _rust_wh(rust_root, rust_source).returncode == 0
    (tmp_path / "rust-src" / "notes.md").write_text("# Changed\n")
    (tmp_path / "oracle-src" / "notes.md").write_text("# Changed\n")
    with pytest.raises(Exception) as caught:
        _python_wh(oracle_root, oracle_source)
    assert "immutable handoff sidecar" in str(caught.value)
    result = _rust_wh(rust_root, rust_source)
    assert result.returncode == 1
    assert "immutable handoff sidecar" in result.stderr


def test_write_handoff_non_utf8_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    rust_source = _source(tmp_path / "rust-src")
    oracle_source = _source(tmp_path / "oracle-src")
    rust_source.write_bytes(b"\xff\xfe")
    oracle_source.write_bytes(b"\xff\xfe")
    with pytest.raises(Exception) as caught:
        _python_wh(oracle_root, oracle_source)
    assert "is not UTF-8" in str(caught.value)
    result = _rust_wh(rust_root, rust_source)
    assert result.returncode == 1
    assert "is not UTF-8" in result.stderr
    assert _sidecar(rust_root) is None


def test_write_handoff_inside_git_tree_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    source = rust_root / "worktrees" / "slot-a" / "repo" / "notes.md"
    source.write_text("# Inside\n")
    result = _rust_wh(rust_root, source)
    assert result.returncode == 1
    assert "inside Git working tree" in result.stderr
    assert _sidecar(rust_root) is None


def test_write_handoff_refuses_without_fixture_attestation(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "plain")
    source = _source(tmp_path / "rust-src")
    result = _rust_wh(rust_root, source, fixture=False)
    assert result.returncode == 1
    assert "fixture-only" in result.stderr
    assert _sidecar(rust_root) is None


def test_write_handoff_schema1_sidecar_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    sidecar = {
        "schema": 1,
        "machine": "node-a",
        "slot": "slot-a",
        "generation": 1,
        "recorded_at": "2026-10-26T01:00:00+00:00",
        "source": "write-handoff",
        "source_identity": {"identity": [1, 2], "size": 3, "sha256": "0" * 64},
        "contents_utf8": "abc",
        "sha256": __import__("hashlib").sha256(b"abc").hexdigest(),
    }
    sidecar["source_identity"] = {
        "identity": [1, 2],
        "size": 3,
        "sha256": sidecar["sha256"],
    }
    for root in (rust_root, oracle_root):
        (root / "worktrees" / SIDECAR_NAME).write_text(json.dumps(sidecar))
    source = _source(tmp_path / "rust-src")
    oracle_source = _source(tmp_path / "oracle-src")
    with pytest.raises(Exception) as caught:
        _python_wh(oracle_root, oracle_source)
    assert "schema 1 lacks auditable source provenance" in str(caught.value)
    result = _rust_wh(rust_root, source)
    assert result.returncode == 1
    assert "schema 1 lacks auditable source provenance" in result.stderr


def test_write_handoff_legacy_file_disagreeing_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    for root in (rust_root, oracle_root):
        (root / "worktrees" / "slot-a" / "HANDOFF.md").write_text("# Other bytes\n")
    source = _source(tmp_path / "rust-src")
    oracle_source = _source(tmp_path / "oracle-src")
    with pytest.raises(Exception) as caught:
        _python_wh(oracle_root, oracle_source)
    assert "legacy HANDOFF.md disagrees" in str(caught.value)
    result = _rust_wh(rust_root, source)
    assert result.returncode == 1
    assert "legacy HANDOFF.md disagrees" in result.stderr
    assert _sidecar(rust_root) is None


def test_write_handoff_legacy_file_agreeing_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    for root in (rust_root, oracle_root):
        (root / "worktrees" / "slot-a" / "HANDOFF.md").write_text("# Handoff\n\nAll done.\n")
    source = _source(tmp_path / "rust-src")
    oracle_source = _source(tmp_path / "oracle-src")
    assert _python_wh(oracle_root, oracle_source) == 0
    result = _rust_wh(rust_root, source)
    assert result.returncode == 0, result.stderr
    assert _sidecar(rust_root) is not None


def _sidecar_gen(root: Path, generation: int) -> dict[str, object] | None:
    path = root / "worktrees" / f"HANDOFF.6.node-a.6.slot-a.{generation}.json"
    if not path.exists():
        return None
    doc: dict[str, object] = json.loads(path.read_text())
    doc.pop("recorded_at")
    identity = cast(dict[str, object], doc["source_identity"])
    identity.pop("identity")
    return doc


def _generation(root: Path) -> int:
    doc: dict[str, object] = json.loads((root / "worktrees" / "ACTIVE.node-a.json").read_text())
    slots = cast(list[dict[str, object]], doc["slots"])
    return int(cast(int, slots[0]["generation"]))


def _supersede_events(root: Path) -> list[dict[str, object]]:
    found: list[dict[str, object]] = []
    for event in _events(root):
        payload = event.get("payload")
        if isinstance(payload, dict) and payload.get("action") == "handoff-checkpoint-superseded":
            found.append(payload)
    return found


def _rust_wh2(
    root: Path,
    source: Path,
    generation: int,
    *,
    supersede: bool = False,
    reason: str | None = None,
    agent: str = "agent-slot-a",
    owner_pid: int | None = None,
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    argv = [
        str(rust_wrkslotsd()), "write-handoff", "slot-a", "--agent", agent,
        "--owner-pid", str(os.getpid() if owner_pid is None else owner_pid),
        "--expected-generation", str(generation),
        "--from-file", str(source), "--root", str(root),
    ]
    if supersede:
        argv.append("--supersede-own-checkpoint")
    if reason is not None:
        argv.extend(["--reason", reason])
    return subprocess.run(
        argv, cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _python_wh2(
    root: Path,
    source: Path,
    generation: int,
    *,
    supersede: bool = False,
    reason: str | None = None,
    agent: str = "agent-slot-a",
    owner_pid: int | None = None,
) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot="slot-a", agent=agent,
        owner_pid=os.getpid() if owner_pid is None else owner_pid,
        expected_generation=generation, from_file=str(source),
        supersede_own_checkpoint=supersede, reason=reason, wait_lock=0.0,
    )
    return wrkslots_cli._cmd_write_handoff(args)


def _source_text(base: Path, text: str) -> Path:
    base.mkdir(parents=True, exist_ok=True)
    path = base / "notes.md"
    path.write_text(text)
    return path


def _python_read(root: Path) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot="slot-a",
        coordinator_pid=os.getpid(), coordinator_authorized=False,
        adopt_legacy_change=False, expected_generation=None, wait_lock=0.0,
    )
    return wrkslots_cli._cmd_read_handoff(args)


def _python_release(root: Path, generation: int) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot="slot-a", agent="agent-slot-a",
        owner_pid=os.getpid(), expected_generation=generation, wait_lock=0.0,
    )
    return wrkslots_cli._cmd_release(args)


def test_write_handoff_supersede_own_checkpoint_matches_python(tmp_path: Path) -> None:
    """#1975 phase 6: a flagged final write supersedes the checkpoint."""
    rust_root, oracle_root = _pair(tmp_path)
    checkpoint_rust = _source(tmp_path / "rust-check")
    checkpoint_oracle = _source(tmp_path / "oracle-check")
    assert _python_wh(oracle_root, checkpoint_oracle) == 0
    assert _rust_wh(rust_root, checkpoint_rust).returncode == 0
    checkpoint_bytes = {
        root: (root / "worktrees" / SIDECAR_NAME).read_bytes()
        for root in (rust_root, oracle_root)
    }
    final_rust = _source_text(tmp_path / "rust-final", "# Final\n\nDone for real.\n")
    final_oracle = _source_text(tmp_path / "oracle-final", "# Final\n\nDone for real.\n")
    assert _python_wh2(oracle_root, final_oracle, 1, supersede=True, reason="final after reboot") == 0
    result = _rust_wh2(rust_root, final_rust, 1, supersede=True, reason="final after reboot")
    assert result.returncode == 0, result.stderr
    assert _generation(rust_root) == 2 and _generation(oracle_root) == 2
    for root in (rust_root, oracle_root):
        # The superseded checkpoint stays on disk byte-identical.
        assert (root / "worktrees" / SIDECAR_NAME).read_bytes() == checkpoint_bytes[root]
        final_doc = _sidecar_gen(root, 2)
        assert final_doc is not None
        assert final_doc["contents_utf8"] == "# Final\n\nDone for real.\n"
        assert final_doc["generation"] == 2
    rust_doc = _sidecar_gen(rust_root, 2)
    oracle_doc = _sidecar_gen(oracle_root, 2)
    assert rust_doc is not None and oracle_doc is not None
    assert rust_doc.pop("source_path") is not None
    oracle_doc.pop("source_path")
    assert rust_doc == oracle_doc
    for root in (rust_root, oracle_root):
        superseded = _supersede_events(root)
        assert len(superseded) == 1
        evidence = cast(dict[str, object], superseded[0]["evidence"])
        assert evidence["previous_generation"] == 1
        assert evidence["reason"] == "final after reboot"
        assert evidence["superseded_sidecar_sha256"] == cast(
            dict[str, object], _sidecar_gen(root, 1)
        )["sha256"]
        assert isinstance(evidence["superseded_intent_sequence"], int)
    assert [event["kind"] for event in _events(rust_root)] == [
        event["kind"] for event in _events(oracle_root)
    ]


def test_write_handoff_supersede_chain_and_unflagged_refusal(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    assert _python_wh(oracle_root, _source(tmp_path / "oracle-c")) == 0
    assert _rust_wh(rust_root, _source(tmp_path / "rust-c")).returncode == 0
    second_rust = _source_text(tmp_path / "rust-2", "# Second\n")
    second_oracle = _source_text(tmp_path / "oracle-2", "# Second\n")
    assert _python_wh2(oracle_root, second_oracle, 1, supersede=True, reason="checkpoint two") == 0
    assert _rust_wh2(rust_root, second_rust, 1, supersede=True, reason="checkpoint two").returncode == 0
    assert _generation(rust_root) == 2 and _generation(oracle_root) == 2
    # Unflagged different bytes still refuse at the new generation.
    third_rust = _source_text(tmp_path / "rust-3", "# Third\n")
    third_oracle = _source_text(tmp_path / "oracle-3", "# Third\n")
    with pytest.raises(Exception) as caught:
        _python_wh2(oracle_root, third_oracle, 2)
    assert "immutable handoff sidecar" in str(caught.value)
    refused = _rust_wh2(rust_root, third_rust, 2)
    assert refused.returncode == 1 and "immutable handoff sidecar" in refused.stderr
    assert _python_wh2(oracle_root, third_oracle, 2, supersede=True, reason="final") == 0
    assert _rust_wh2(rust_root, third_rust, 2, supersede=True, reason="final").returncode == 0
    assert _generation(rust_root) == 3 and _generation(oracle_root) == 3
    assert len(_supersede_events(rust_root)) == 2
    assert len(_supersede_events(oracle_root)) == 2


def test_write_handoff_supersede_after_read_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    assert _python_wh(oracle_root, _source(tmp_path / "oracle-c")) == 0
    assert _rust_wh(rust_root, _source(tmp_path / "rust-c")).returncode == 0
    assert _python_read(oracle_root) == 0
    assert _python_read(rust_root) == 0
    final_oracle = _source_text(tmp_path / "oracle-f", "# Final\n")
    final_rust = _source_text(tmp_path / "rust-f", "# Final\n")
    with pytest.raises(Exception) as caught:
        _python_wh2(oracle_root, final_oracle, 1, supersede=True, reason="too late")
    assert "immutable once read or released" in str(caught.value)
    result = _rust_wh2(rust_root, final_rust, 1, supersede=True, reason="too late")
    assert result.returncode == 1 and "immutable once read or released" in result.stderr
    assert _generation(rust_root) == 1 and _generation(oracle_root) == 1
    assert _supersede_events(rust_root) == [] and _supersede_events(oracle_root) == []


def test_write_handoff_supersede_after_release_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    assert _python_wh(oracle_root, _source(tmp_path / "oracle-c")) == 0
    assert _rust_wh(rust_root, _source(tmp_path / "rust-c")).returncode == 0
    assert _python_release(oracle_root, 1) == 0
    assert _python_release(rust_root, 1) == 0
    final_oracle = _source_text(tmp_path / "oracle-f", "# Final\n")
    final_rust = _source_text(tmp_path / "rust-f", "# Final\n")
    with pytest.raises(Exception) as caught:
        _python_wh2(oracle_root, final_oracle, 1, supersede=True, reason="too late")
    assert "immutable once read or released" in str(caught.value)
    result = _rust_wh2(rust_root, final_rust, 1, supersede=True, reason="too late")
    assert result.returncode == 1 and "immutable once read or released" in result.stderr
    assert _generation(rust_root) == 1 and _generation(oracle_root) == 1


def test_write_handoff_supersede_legacy_disagreement_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    assert _python_wh(oracle_root, _source(tmp_path / "oracle-c")) == 0
    assert _rust_wh(rust_root, _source(tmp_path / "rust-c")).returncode == 0
    for root in (rust_root, oracle_root):
        (root / "worktrees" / "slot-a" / "HANDOFF.md").write_text("# Other bytes\n")
    final_oracle = _source_text(tmp_path / "oracle-f", "# Final\n")
    final_rust = _source_text(tmp_path / "rust-f", "# Final\n")
    with pytest.raises(Exception) as caught:
        _python_wh2(oracle_root, final_oracle, 1, supersede=True, reason="final")
    assert "legacy HANDOFF.md disagrees" in str(caught.value)
    result = _rust_wh2(rust_root, final_rust, 1, supersede=True, reason="final")
    assert result.returncode == 1 and "legacy HANDOFF.md disagrees" in result.stderr
    assert _generation(rust_root) == 1 and _generation(oracle_root) == 1
    assert _supersede_events(rust_root) == [] and _supersede_events(oracle_root) == []


def test_write_handoff_supersede_schema1_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    sidecar = {
        "schema": 1,
        "machine": "node-a",
        "slot": "slot-a",
        "generation": 1,
        "recorded_at": "2026-10-26T01:00:00+00:00",
        "source": "write-handoff",
        "source_identity": {"identity": [1, 2], "size": 3, "sha256": "0" * 64},
        "contents_utf8": "abc",
        "sha256": __import__("hashlib").sha256(b"abc").hexdigest(),
    }
    sidecar["source_identity"] = {
        "identity": [1, 2],
        "size": 3,
        "sha256": sidecar["sha256"],
    }
    for root in (rust_root, oracle_root):
        (root / "worktrees" / SIDECAR_NAME).write_text(json.dumps(sidecar))
    source = _source(tmp_path / "rust-src")
    oracle_source = _source(tmp_path / "oracle-src")
    with pytest.raises(Exception) as caught:
        _python_wh2(oracle_root, oracle_source, 1, supersede=True, reason="upgrade")
    assert "schema 1 lacks auditable source provenance" in str(caught.value)
    result = _rust_wh2(rust_root, source, 1, supersede=True, reason="upgrade")
    assert result.returncode == 1
    assert "schema 1 lacks auditable source provenance" in result.stderr
    assert _generation(rust_root) == 1 and _generation(oracle_root) == 1


def test_write_handoff_supersede_auth_refusals_change_nothing(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    assert _python_wh(oracle_root, _source(tmp_path / "oracle-c")) == 0
    assert _rust_wh(rust_root, _source(tmp_path / "rust-c")).returncode == 0

    def snapshot(root: Path) -> tuple[bytes, int]:
        active = (root / "worktrees" / "ACTIVE.node-a.json").read_bytes()
        return active, len(_events(root))

    before = {root: snapshot(root) for root in (rust_root, oracle_root)}
    final_oracle = _source_text(tmp_path / "oracle-f", "# Final\n")
    final_rust = _source_text(tmp_path / "rust-f", "# Final\n")
    # Missing and blank --reason refuse before any bump.
    with pytest.raises(Exception) as caught:
        _python_wh2(oracle_root, final_oracle, 1, supersede=True)
    assert "requires a non-empty --reason" in str(caught.value)
    missing = _rust_wh2(rust_root, final_rust, 1, supersede=True)
    assert missing.returncode == 1 and "requires a non-empty --reason" in missing.stderr
    with pytest.raises(Exception) as caught:
        _python_wh2(oracle_root, final_oracle, 1, supersede=True, reason="  ")
    assert "requires a non-empty --reason" in str(caught.value)
    # Wrong agent and stale generation refuse with the shared strings.
    with pytest.raises(Exception) as caught:
        _python_wh2(oracle_root, final_oracle, 1, supersede=True, reason="x", agent="other")
    assert "ownership mismatch" in str(caught.value)
    wrong_agent = _rust_wh2(rust_root, final_rust, 1, supersede=True, reason="x", agent="other")
    assert wrong_agent.returncode == 1 and "ownership mismatch" in wrong_agent.stderr
    with pytest.raises(Exception) as caught:
        _python_wh2(oracle_root, final_oracle, 9, supersede=True, reason="x")
    assert "stale generation" in str(caught.value)
    stale = _rust_wh2(rust_root, final_rust, 9, supersede=True, reason="x")
    assert stale.returncode == 1 and "stale generation" in stale.stderr
    # A wrong owner PID refuses in both editions before any bump (the
    # exact refusal wording follows each edition's owner proof).
    with pytest.raises(Exception):
        _python_wh2(oracle_root, final_oracle, 1, supersede=True, reason="x", owner_pid=1)
    wrong_owner = _rust_wh2(rust_root, final_rust, 1, supersede=True, reason="x", owner_pid=1)
    assert wrong_owner.returncode == 1
    for root in (rust_root, oracle_root):
        assert snapshot(root) == before[root]


def _event_log_bytes(root: Path) -> dict[str, bytes]:
    directory = root / "worktrees" / "EVENTS.node-a"
    if not directory.is_dir():
        return {}
    return {
        path.name: path.read_bytes()
        for path in sorted(directory.iterdir())
        if path.is_file()
    }


def _sidecar_snapshot(root: Path) -> tuple[bytes, int]:
    path = root / "worktrees" / SIDECAR_NAME
    return path.read_bytes(), path.stat().st_ino


def test_write_handoff_supersede_identical_bytes_is_idempotent(
    tmp_path: Path,
) -> None:
    """Flagged byte-identical re-write completes in place with no bump."""
    rust_root, oracle_root = _pair(tmp_path)
    checkpoint_rust = _source(tmp_path / "rust-check")
    checkpoint_oracle = _source(tmp_path / "oracle-check")
    assert _python_wh(oracle_root, checkpoint_oracle) == 0
    assert _rust_wh(rust_root, checkpoint_rust).returncode == 0
    for root, source in (
        (oracle_root, checkpoint_oracle),
        (rust_root, checkpoint_rust),
    ):
        before_events = _event_log_bytes(root)
        before_sidecar = _sidecar_snapshot(root)
        before_active = (root / "worktrees" / "ACTIVE.node-a.json").read_bytes()
        assert _generation(root) == 1
        # Twice: retries must not churn generations or events.
        for _ in range(2):
            if root is oracle_root:
                assert (
                    _python_wh2(
                        root, source, 1, supersede=True, reason="retry same bytes"
                    )
                    == 0
                )
            else:
                result = _rust_wh2(
                    root, source, 1, supersede=True, reason="retry same bytes"
                )
                assert result.returncode == 0, result.stderr
            assert _generation(root) == 1
            assert _event_log_bytes(root) == before_events
            assert _sidecar_snapshot(root) == before_sidecar
            assert (root / "worktrees" / "ACTIVE.node-a.json").read_bytes() == (
                before_active
            )
            assert _supersede_events(root) == []


def test_write_handoff_supersede_identical_after_read_matches_unflagged(
    tmp_path: Path,
) -> None:
    """After a read, flagged identical follows today's unflagged identical."""
    rust_root, oracle_root = _pair(tmp_path)
    checkpoint_rust = _source(tmp_path / "rust-check")
    checkpoint_oracle = _source(tmp_path / "oracle-check")
    assert _python_wh(oracle_root, checkpoint_oracle) == 0
    assert _rust_wh(rust_root, checkpoint_rust).returncode == 0
    assert _python_read(oracle_root) == 0
    assert _python_read(rust_root) == 0
    # Pin today's unflagged identical behaviour after a read: it succeeds.
    assert _python_wh(oracle_root, checkpoint_oracle) == 0
    assert _rust_wh(rust_root, checkpoint_rust).returncode == 0
    assert _generation(rust_root) == 1 and _generation(oracle_root) == 1
    for root, source in (
        (oracle_root, checkpoint_oracle),
        (rust_root, checkpoint_rust),
    ):
        before_events = _event_log_bytes(root)
        before_sidecar = _sidecar_snapshot(root)
        before_active = (root / "worktrees" / "ACTIVE.node-a.json").read_bytes()
        if root is oracle_root:
            assert (
                _python_wh2(root, source, 1, supersede=True, reason="retry after read")
                == 0
            )
        else:
            result = _rust_wh2(
                root, source, 1, supersede=True, reason="retry after read"
            )
            assert result.returncode == 0, result.stderr
        assert _generation(root) == 1
        assert _event_log_bytes(root) == before_events
        assert _sidecar_snapshot(root) == before_sidecar
        assert (root / "worktrees" / "ACTIVE.node-a.json").read_bytes() == (
            before_active
        )
        assert _supersede_events(root) == []


_CHECKPOINT_WRITER = (
    "import json, os, subprocess, sys\n"
    "from pathlib import Path\n"
    "from wrkslots import cli\n"
    "root, source, edition, rust = sys.argv[1:5]\n"
    "active = Path(root) / 'worktrees' / 'ACTIVE.node-a.json'\n"
    "doc = json.loads(active.read_text())\n"
    "identity = cli._read_process_identity(os.getpid())\n"
    "me = {'pid': identity.pid, 'start_ticks': identity.start_ticks,"
    " 'boot_id': identity.boot_id, 'host_id': identity.host_id,"
    " 'cgroup_path': identity.cgroup_path}\n"
    "doc['slots'][0]['owner'] = me\n"
    "doc['slots'][0]['coordinator_lease'] = me\n"
    "active.write_text(json.dumps(doc))\n"
    "env = dict(os.environ)\n"
    "env['WRKSLOTSD_FIXTURE_WRITER'] = '1'\n"
    "common = ['slot-a', '--agent', 'agent-slot-a', '--owner-pid', str(os.getpid()),"
    " '--expected-generation', '1', '--from-file', source]\n"
    "argv = ([rust, 'write-handoff', *common, '--root', root] if edition == 'rust'"
    " else [sys.executable, '-m', 'wrkslots', '--project-root', root,"
    " 'write-handoff', *common])\n"
    "result = subprocess.run(argv, capture_output=True, text=True, env=env, timeout=900)\n"
    "sys.stdout.write(result.stdout)\n"
    "sys.stderr.write(result.stderr)\n"
    "raise SystemExit(result.returncode)\n"
)

_SUCCESSOR_WRITER = (
    "import os, subprocess, sys\n"
    "root, checkpoint, final, edition, rust = sys.argv[1:6]\n"
    "pid = str(os.getpid())\n"
    "env = dict(os.environ)\n"
    "env['WRKSLOTSD_FIXTURE_WRITER'] = '1'\n"
    "def run(argv):\n"
    "    result = subprocess.run(argv, capture_output=True, text=True, env=env, timeout=900)\n"
    "    sys.stdout.write(result.stdout)\n"
    "    sys.stderr.write(result.stderr)\n"
    "    return result.returncode\n"
    "def edition_argv(command, extra):\n"
    "    if edition == 'rust':\n"
    "        return [rust, command, *extra, '--root', root]\n"
    "    return [sys.executable, '-m', 'wrkslots', '--project-root', root, command, *extra]\n"
    "transfer = ['slot-a', '--agent', 'agent-slot-a', '--owner-pid', pid,"
    " '--expected-generation', '1']\n"
    "code = run(edition_argv('transfer-owner', transfer))\n"
    "if code != 0:\n"
    "    raise SystemExit(code)\n"
    "plain = ['slot-a', '--agent', 'agent-slot-a', '--owner-pid', pid,"
    " '--expected-generation', '1', '--from-file', checkpoint]\n"
    "code = run(edition_argv('write-handoff', plain))\n"
    "if code != 0:\n"
    "    raise SystemExit(code)\n"
    "flagged = [*plain[:-1], final, '--supersede-own-checkpoint', '--reason',"
    " 'final after reboot']\n"
    "raise SystemExit(run(edition_argv('write-handoff', flagged)))\n"
)


def _run_owner_helper(
    script: str, *args: str
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["PYTHONPATH"] = str(AU_ROOT / "py")
    env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    return subprocess.run(
        [sys.executable, "-c", script, *args],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def test_write_handoff_supersede_after_transfer_to_different_successor(
    tmp_path: Path,
) -> None:
    """Checkpoint by one live process, superseded by a different successor."""
    rust_root, oracle_root = _pair(tmp_path)
    checkpoint_rust = _source(tmp_path / "rust-check")
    checkpoint_oracle = _source(tmp_path / "oracle-check")
    rust_binary = str(rust_wrkslotsd())
    for root, source, edition in (
        (oracle_root, checkpoint_oracle, "python"),
        (rust_root, checkpoint_rust, "rust"),
    ):
        result = _run_owner_helper(
            _CHECKPOINT_WRITER, str(root), str(source), edition, rust_binary
        )
        assert result.returncode == 0, result.stderr
        assert _generation(root) == 1
    final_rust = _source_text(tmp_path / "rust-final", "# Final\n\nDone for real.\n")
    final_oracle = _source_text(tmp_path / "oracle-final", "# Final\n\nDone for real.\n")
    for root, checkpoint, final, edition in (
        (oracle_root, checkpoint_oracle, final_oracle, "python"),
        (rust_root, checkpoint_rust, final_rust, "rust"),
    ):
        result = _run_owner_helper(
            _SUCCESSOR_WRITER,
            str(root), str(checkpoint), str(final), edition, rust_binary,
        )
        assert result.returncode == 0, (result.stdout, result.stderr)
        assert _generation(root) == 2
        assert _sidecar_gen(root, 1) is not None
        final_doc = _sidecar_gen(root, 2)
        assert final_doc is not None
        assert final_doc["contents_utf8"] == "# Final\n\nDone for real.\n"
        superseded = _supersede_events(root)
        assert len(superseded) == 1
        evidence = cast(dict[str, object], superseded[0]["evidence"])
        assert evidence["previous_generation"] == 1
        assert evidence["reason"] == "final after reboot"
    assert [event["kind"] for event in _events(rust_root)] == [
        event["kind"] for event in _events(oracle_root)
    ]
    rust_doc = _sidecar_gen(rust_root, 2)
    oracle_doc = _sidecar_gen(oracle_root, 2)
    assert rust_doc is not None and oracle_doc is not None
    assert rust_doc.pop("source_path") is not None
    oracle_doc.pop("source_path")
    assert rust_doc == oracle_doc


def _rust_read(root: Path) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    return subprocess.run(
        [str(rust_wrkslotsd()), "read-handoff", "slot-a",
         "--coordinator-pid", str(os.getpid()), "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _python_cli_crash_after_supersede_bump(
    root: Path, source: Path
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["PYTHONPATH"] = str(AU_ROOT / "py")
    env["WRKSLOTS_TEST_INTERRUPT"] = "after-handoff-checkpoint-superseded"
    return subprocess.run(
        [sys.executable, "-m", "wrkslots", "--project-root", str(root),
         "write-handoff", "slot-a", "--agent", "agent-slot-a",
         "--owner-pid", str(os.getpid()), "--expected-generation", "1",
         "--from-file", str(source), "--supersede-own-checkpoint",
         "--reason", "final after reboot"],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def test_read_handoff_after_supersede_bump_crash_names_checkpoint(
    tmp_path: Path,
) -> None:
    """A sidecar-less bumped generation's read refusal names the checkpoint."""
    rust_root, oracle_root = _pair(tmp_path)
    checkpoint_rust = _source(tmp_path / "rust-check")
    checkpoint_oracle = _source(tmp_path / "oracle-check")
    assert _python_wh(oracle_root, checkpoint_oracle) == 0
    assert _rust_wh(rust_root, checkpoint_rust).returncode == 0
    checkpoint_sha = {
        root: cast(dict[str, object], _sidecar_gen(root, 1))["sha256"]
        for root in (rust_root, oracle_root)
    }
    final_rust = _source_text(tmp_path / "rust-final", "# Final\n\nDone for real.\n")
    final_oracle = _source_text(tmp_path / "oracle-final", "# Final\n\nDone for real.\n")
    for root, final in ((oracle_root, final_oracle), (rust_root, final_rust)):
        crashed = _python_cli_crash_after_supersede_bump(root, final)
        assert crashed.returncode == 86, (crashed.stdout, crashed.stderr)
        assert _generation(root) == 2
        assert _sidecar_gen(root, 2) is None
    with pytest.raises(Exception) as caught:
        _python_read(oracle_root)
    assert str(checkpoint_sha[oracle_root]) in str(caught.value)
    assert SIDECAR_NAME in str(caught.value)
    result = _rust_read(rust_root)
    assert result.returncode == 1, (result.stdout, result.stderr)
    assert str(checkpoint_sha[rust_root]) in result.stderr
    assert SIDECAR_NAME in result.stderr
    # The exact owner's flagged retry at the bumped generation converges.
    assert _python_wh2(oracle_root, final_oracle, 2, supersede=True, reason="final after reboot") == 0
    assert _rust_wh2(rust_root, final_rust, 2, supersede=True, reason="final after reboot").returncode == 0
    assert _generation(rust_root) == 2 and _generation(oracle_root) == 2
    assert cast(dict[str, object], _sidecar_gen(rust_root, 2))["contents_utf8"] == (
        "# Final\n\nDone for real.\n"
    )
    assert len(_supersede_events(rust_root)) == 1
    assert len(_supersede_events(oracle_root)) == 1


def test_flagged_registry_reads_with_pinned_older_reader(tmp_path: Path) -> None:
    """#1975 phase 6 compat: the pinned older reader parses a superseded registry."""
    import sys

    root = _fixture(tmp_path / "scratch")
    assert _python_wh(root, _source(tmp_path / "check-src")) == 0
    final_source = _source_text(tmp_path / "final-src", "# Final\n\nDone for real.\n")
    assert _python_wh2(root, final_source, 1, supersede=True, reason="final after reboot") == 0
    assert _generation(root) == 2
    control = _fixture(tmp_path / "control")
    assert _python_wh(control, _source(tmp_path / "control-src")) == 0
    assert _generation(control) == 1
    pinned = "096a8d74"
    probe = subprocess.run(
        ["git", "-C", str(AU_ROOT), "cat-file", "-t", pinned],
        capture_output=True, text=True, check=False, timeout=60,
    )
    if probe.returncode != 0:
        pytest.skip(f"pinned revision {pinned} is unavailable in this checkout")
    old_tree = tmp_path / "old-reader"
    added = subprocess.run(
        ["git", "-C", str(AU_ROOT), "worktree", "add", "--detach", str(old_tree), pinned],
        capture_output=True, text=True, check=False, timeout=300,
    )
    if added.returncode != 0:
        pytest.skip(f"cannot materialise pinned revision {pinned}: {added.stderr.strip()}")
    try:
        env = dict(os.environ)
        env["PYTHONPATH"] = str(old_tree / "py")
        # Doctor/audit on the flagged registry must match the unflagged
        # control exactly in return code, with no line naming the action.
        for argv in (["doctor"], ["audit"]):
            flagged = subprocess.run(
                [sys.executable, "-m", "wrkslots", "--project-root", str(root), *argv],
                capture_output=True, text=True, check=False, timeout=300, env=env,
            )
            unflagged = subprocess.run(
                [sys.executable, "-m", "wrkslots", "--project-root", str(control), *argv],
                capture_output=True, text=True, check=False, timeout=300, env=env,
            )
            assert flagged.returncode == unflagged.returncode, (
                argv, flagged.returncode, unflagged.returncode,
                flagged.stdout, flagged.stderr, unflagged.stdout, unflagged.stderr,
            )
            assert "handoff-checkpoint-superseded" not in (
                flagged.stdout + flagged.stderr
            )
        for argv in (
            ["status"],
            ["read-handoff", "slot-a", "--coordinator-pid", str(os.getpid())],
            ["retirement-queue", "--format", "json"],
        ):
            result = subprocess.run(
                [sys.executable, "-m", "wrkslots", "--project-root", str(root), *argv],
                capture_output=True, text=True, check=False, timeout=300, env=env,
            )
            assert result.returncode == 0, (argv, result.stderr, result.stdout)
            if argv[0] == "read-handoff":
                assert "Done for real." in result.stdout
    finally:
        subprocess.run(
            ["git", "-C", str(AU_ROOT), "worktree", "remove", "--force", str(old_tree)],
            capture_output=True, text=True, check=False, timeout=300,
        )

