"""A dead FUSE entry in one slot must not refuse other slots' commands (#1802).

A pre-#1730 Eden validate slot kept its FUSE mount inside the slot; a
failed remove left dead copies (stat fails with ENOTCONN, "Transport
endpoint is not connected") in sandbox mount namespaces.  Every
heartbeat, create and classify-create-journals whose namespace saw
one such entry refused host-wide, although the entry belongs to
another slot.

These tests are deterministic: a fake mountinfo table lists the FUSE
entries and a monkeypatched stat/lstat raises the dead-mount errno
for the chosen paths.  No real FUSE mount is created.
"""

from __future__ import annotations

import argparse
import errno
import json
import os
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli, slotimage

DIGEST = "b" * 64
DEAD_ERRNOS = [errno.ENOTCONN, errno.EIO]


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


def _identity() -> dict[str, object]:
    identity = cli._read_process_identity(os.getpid())
    return {
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    }


def _record(slot: str, agent: str, branch: str, head: str) -> dict[str, object]:
    identity = _identity()
    return {
        "slot": slot, "agent": agent, "task": "do work", "purpose": "testing",
        "machine": "node-a", "generation": 1,
        "created_at": "2026-10-26T00:00:00Z", "heartbeat_at": "2026-10-26T01:00:00Z",
        "heartbeat_ttl_seconds": 6000, "owner": identity, "coordinator_lease": identity,
        "coordinator_recovery_note": None, "handoff": None,
        "checkouts": [{
            "name": "repo", "path": f"worktrees/{slot}/repo",
            "repository": "../upstream-repo", "branch": branch, "start_point": head,
            "remote": "origin", "remote_url_sha256": DIGEST,
            "landed_ref": "refs/remotes/origin/main", "head": head,
            "containing_remote_refs": ["refs/remotes/origin/main"], "vcs": "git",
        }],
    }


def _fixture(base: Path) -> Path:
    """A project with two healthy slots, slot-a and slot-b."""

    root = base / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\nmachine: node-a\nworktrees_dir: worktrees\n"
        "default_remote: origin\ndefault_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 6000\nliveness_command: live.py\n"
    )
    upstream = base / "upstream-repo"
    head = _repo(upstream)
    _git(upstream, "remote", "add", "origin", str(upstream))
    _git(upstream, "fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*")
    for slot, branch in (("slot-a", "suite/work-a"), ("slot-b", "suite/work-b")):
        _git(
            upstream, "worktree", "add", "-q", "-b", branch,
            str(root / "worktrees" / slot / "repo"), "HEAD",
        )
    records = [
        _record("slot-a", "agent-slot-a", "suite/work-a", head),
        _record("slot-b", "agent-slot-b", "suite/work-b", head),
    ]
    (root / "worktrees" / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 7, "slots": records})
    )
    (root / "worktrees" / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 3, "records": []})
    )
    return root


def _kill(
    monkeypatch: pytest.MonkeyPatch, base: Path, dead_paths: list[Path], error_number: int
) -> None:
    """Make stat/lstat of `dead_paths` raise `error_number`, with a fake table."""

    deads = tuple(str(path.absolute()) for path in dead_paths)
    table = "24 1 0:22 / / rw shared:1 - ext4 /dev/root rw\n" + "".join(
        f"{100 + index} 24 0:{276 + index} / {path} rw,nosuid,nodev "
        f"shared:{9 + index} - fuse edenfs: rw,user_id=1000\n"
        for index, path in enumerate(deads)
    )
    fake = base / "fake-mountinfo"
    fake.write_text(table)
    monkeypatch.setattr(cli, "_SELF_MOUNTINFO", fake)
    monkeypatch.setattr(
        slotimage, "mount_table", lambda mountinfo=None: slotimage._parse_mount_table(table)
    )
    real_stat, real_lstat = os.stat, os.lstat

    def is_dead(value: object) -> bool:
        return isinstance(value, str) and any(
            value == dead or value.startswith(dead + "/") for dead in deads
        )

    def dead_stat(path: str, *args: object, **kwargs: object) -> os.stat_result:
        name = os.fspath(path)
        if is_dead(name):
            raise OSError(error_number, os.strerror(error_number), name)
        return real_stat(path, *args, **kwargs)  # type: ignore[arg-type]

    def dead_lstat(path: str, *args: object, **kwargs: object) -> os.stat_result:
        name = os.fspath(path)
        if is_dead(name):
            raise OSError(error_number, os.strerror(error_number), name)
        return real_lstat(path, *args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(os, "stat", dead_stat)
    monkeypatch.setattr(os, "lstat", dead_lstat)


def _heartbeat(root: Path, slot: str, agent: str) -> int:
    return cli._cmd_heartbeat(
        argparse.Namespace(
            project_root=str(root), machine=None, slot=slot, agent=agent,
            owner_pid=os.getpid(), expected_generation=1, wait_lock=0.0,
        )
    )


def _classify(root: Path, slot: str, agent: str) -> int:
    return cli._cmd_classify_create_journals(
        argparse.Namespace(
            project_root=str(root), machine=None, slot=slot, agent=agent,
            slot_type="agent", all_kinds=False, all_machines=False, format="human",
        )
    )


def _create(root: Path, slot: str, agent: str, branch: str) -> int:
    return cli._cmd_create(
        argparse.Namespace(
            project_root=str(root), machine="node-a", slot=slot, slot_type="agent",
            coordinator_authorized=True, agent=agent, task="task-1", purpose="testing",
            owner_pid=os.getpid(), coordinator_pid=os.getpid(),
            task_scope_unit=None, task_scope_invocation_id=None,
            repo=["repo=../upstream-repo"], remote=[], remote_url=[],
            branch=[f"repo={branch}"], start=[], override_disk_floor=False,
            representation="config", format="human", wait_lock=0.0,
        )
    )


def _revision(root: Path) -> int:
    document = json.loads((root / "worktrees" / "ACTIVE.node-a.json").read_text())
    return int(document["revision"])


@pytest.mark.parametrize("error_number", DEAD_ERRNOS)
def test_heartbeat_records_with_a_finding_on_another_slots_dead_entry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
    error_number: int,
) -> None:
    root = _fixture(tmp_path)
    dead = root / "worktrees" / "slot-b" / "repo"
    _kill(monkeypatch, tmp_path, [dead], error_number)

    assert _heartbeat(root, "slot-a", "agent-slot-a") == 0

    assert _revision(root) == 8
    assert f"dead FUSE entry at {dead}, owning slot slot-b" in capsys.readouterr().err


@pytest.mark.parametrize("error_number", DEAD_ERRNOS)
def test_heartbeat_refuses_on_its_own_slots_dead_entry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, error_number: int
) -> None:
    root = _fixture(tmp_path)
    dead = root / "worktrees" / "slot-b" / "repo"
    _kill(monkeypatch, tmp_path, [dead], error_number)

    with pytest.raises(cli.Refusal, match=r"dead FUSE entry at .* owning slot slot-b"):
        _heartbeat(root, "slot-b", "agent-slot-b")

    assert _revision(root) == 7


def test_create_proceeds_with_a_finding_on_another_slots_dead_entry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _fixture(tmp_path)
    dead = root / "worktrees" / "slot-b" / "repo"
    _kill(monkeypatch, tmp_path, [dead], errno.ENOTCONN)

    assert _create(root, "slot-c", "agent-new", "suite/new") == 0

    assert f"dead FUSE entry at {dead}, owning slot slot-b" in capsys.readouterr().err
    document = json.loads((root / "worktrees" / "ACTIVE.node-a.json").read_text())
    assert "slot-c" in {record["slot"] for record in document["slots"]}


def test_create_refuses_onto_a_dead_target(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _fixture(tmp_path)
    target = root / "worktrees" / "slot-d"
    target.mkdir()
    _kill(monkeypatch, tmp_path, [target], errno.ENOTCONN)

    with pytest.raises(cli.Refusal, match=r"dead FUSE entry at .* owning slot slot-d"):
        _create(root, "slot-d", "agent-d", "suite/d")

    document = json.loads((root / "worktrees" / "ACTIVE.node-a.json").read_text())
    assert "slot-d" not in {record["slot"] for record in document["slots"]}


def test_classify_proceeds_with_a_finding_on_another_slots_dead_entry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _fixture(tmp_path)
    dead = root / "worktrees" / "slot-b" / "repo"
    _kill(monkeypatch, tmp_path, [dead], errno.EIO)

    assert _classify(root, "slot-a", "agent-slot-a") == 0

    assert f"dead FUSE entry at {dead}, owning slot slot-b" in capsys.readouterr().err


def test_classify_refuses_for_its_own_slots_dead_entry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _fixture(tmp_path)
    dead = root / "worktrees" / "slot-b" / "repo"
    _kill(monkeypatch, tmp_path, [dead], errno.ENOTCONN)

    with pytest.raises(cli.Refusal, match=r"dead FUSE entry at .* owning slot slot-b"):
        _classify(root, "slot-b", "agent-slot-b")


def test_storage_census_reports_the_dead_entry_for_its_owning_slot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _fixture(tmp_path)
    dead = root / "worktrees" / "slot-b" / "repo"
    _kill(monkeypatch, tmp_path, [dead], errno.ENOTCONN)

    config = cli._load_config(str(root), "node-a")
    states, _archives = cli._validate_global_state(config)
    findings = cli._registry_storage_inconsistencies(config, states)

    dead_findings = [item for item in findings if item.kind == "dead-fuse-entry"]
    assert [(item.slot, item.detail) for item in dead_findings] == [
        ("slot-b", f"dead FUSE entry at {dead}, owning slot slot-b")
    ]


def _interrupted_journal(root: Path) -> None:
    """Leave a genuine interrupted create journal for slot-z (failing hook)."""

    config_path = root / ".wrkslots.yml"
    config_path.write_text(
        config_path.read_text()
        + "post_provision_hooks:\n  - 'echo hook-broken >&2; exit 3'\n"
    )
    with pytest.raises(Exception):
        _create(root, "slot-z", "agent-z", "suite/z")
    assert list((root / "worktrees").glob("CREATE.*.journal"))


def test_classify_skips_a_dead_journal_of_another_slot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _fixture(tmp_path)
    _interrupted_journal(root)
    dead = root / "worktrees" / "slot-z" / "repo"
    _kill(monkeypatch, tmp_path, [dead], errno.ENOTCONN)

    assert _classify(root, "slot-a", "agent-slot-a") == 0
    assert f"dead FUSE entry at {dead}, owning slot slot-z" in capsys.readouterr().err


def test_classify_refuses_a_dead_journal_of_the_requested_slot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _fixture(tmp_path)
    _interrupted_journal(root)
    dead = root / "worktrees" / "slot-z" / "repo"
    _kill(monkeypatch, tmp_path, [dead], errno.ENOTCONN)

    with pytest.raises(cli.Refusal, match=r"dead FUSE entry at .* owning slot slot-z"):
        _classify(root, "slot-z", "agent-z")
