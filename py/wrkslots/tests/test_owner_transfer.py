"""`wrkslots transfer-owner`: hand a slot whose owner died to its live successor."""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import (
    active_slots,
    checkout,
    command,
    control_directory,
    create,
    git,
    make_project,
    mark_owner_dead,
    terminate_process,
)


def transfer(project: Path, pid: int, *, agent: str = "codex-1", generation: str = "1") -> subprocess.CompletedProcess[str]:
    return command(
        project, "transfer-owner", "slot01", "--agent", agent, "--owner-pid", str(pid),
        "--expected-generation", generation,
    )


def owner_pid(project: Path) -> object:
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    owner = row["owner"]
    return None if owner is None else owner["pid"]


def last_active_event(project: Path) -> dict[str, object]:
    events = sorted((control_directory(project) / "EVENTS.testhost").glob("*.json"))
    for path in reversed(events):
        document = json.loads(path.read_text(encoding="utf-8"))
        if document["kind"] == "active-state-recorded":
            return dict(document["payload"])
    raise AssertionError("no active-state event")


@pytest.mark.ordinary_environment
def test_a_dead_owner_is_replaced_by_its_successor_and_nothing_else_changes(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    mark_owner_dead(project)
    before = active_slots(project)[0]
    assert isinstance(before, dict)
    moved = transfer(project, os.getpid())
    assert moved.returncode == 0, moved.stderr
    assert "basis=caller-ancestry previous=dead" in moved.stdout
    after = active_slots(project)[0]
    assert isinstance(after, dict)
    assert after["owner"]["pid"] == os.getpid() and after["owner"]["boot_id"] != "finished-boot"
    assert {key: value for key, value in after.items() if key not in ("owner", "heartbeat_at")} == {
        key: value for key, value in before.items() if key not in ("owner", "heartbeat_at")
    }
    event = last_active_event(project)
    assert event.get("action") == "owner-transferred", event
    again = transfer(project, os.getpid())
    assert again.returncode == 0 and "already owned" in again.stdout
    heartbeat = command(project, "heartbeat", "slot01", "--agent", "codex-1", "--owner-pid",
                        str(os.getpid()), "--expected-generation", "1")
    assert heartbeat.returncode == 0, heartbeat.stderr


@pytest.mark.ordinary_environment
def test_a_slot_with_no_recorded_owner_is_bound_to_its_successor(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project, bind_owner=False).returncode == 0
    moved = transfer(project, os.getpid())
    assert moved.returncode == 0, moved.stderr
    assert owner_pid(project) == os.getpid()


@pytest.mark.ordinary_environment
def test_a_live_owner_a_wrong_agent_or_generation_and_an_outside_successor_are_refused(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    sleeper = subprocess.Popen(["sleep", "60"], text=True)
    try:
        live = transfer(project, sleeper.pid)
        assert live.returncode == 3 and "owner is live" in live.stderr, live.stderr
        mark_owner_dead(project)
        assert transfer(project, os.getpid(), agent="someone").returncode == 3
        assert transfer(project, os.getpid(), generation="2").returncode == 3
        outside = transfer(project, sleeper.pid)
        assert outside.returncode == 3, outside.stderr
        assert "neither in the invoking process ancestry nor inside slot slot01's own box" in outside.stderr
        row = active_slots(project)[0]
        assert isinstance(row, dict) and row["owner"]["boot_id"] == "finished-boot"
    finally:
        terminate_process(sleeper)


@pytest.mark.ordinary_environment
def test_transfer_warns_and_proceeds_with_unregistered_nested_worktrees(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    nested = checkout(project) / "ignored" / "nested-a"
    nested.parent.mkdir(parents=True)
    git(checkout(project), "worktree", "add", "--detach", str(nested), "HEAD")
    (nested / "dirty.txt").write_text("live work\n", encoding="utf-8")
    mark_owner_dead(project)
    moved = transfer(project, os.getpid())
    assert moved.returncode == 0, moved.stderr
    assert "nested-a" in moved.stderr
    assert "unregistered nested worktree" in moved.stderr
    assert owner_pid(project) == os.getpid()
    assert nested.is_dir()
    events = sorted((control_directory(project) / "EVENTS.testhost").glob("*.json"))
    payloads = [json.loads(path.read_text(encoding="utf-8")) for path in events]
    transferred = [
        item["payload"] for item in payloads
        if item["kind"] == "active-state-recorded"
        and item["payload"].get("action") == "owner-transferred"
    ]
    assert transferred and "nested-a" in str(
        transferred[-1]["evidence"]["unregistered_nested_worktrees"]
    )


def _release_with_dying_owner(project: Path, tmp_path: Path) -> None:
    source = tmp_path / "release-notes.md"
    source.write_text("# Handoff\n\nAll done.\n", encoding="utf-8")
    helper = (
        "import argparse, dataclasses, os, sys\n"
        "from wrkslots import cli\n"
        "root = sys.argv[1]; source = sys.argv[2]\n"
        "config = cli._load_config(root, None)\n"
        "state = cli._load_active(config)\n"
        "record = cli._find_record(state, 'slot01')\n"
        "identity = cli._read_process_identity(os.getpid())\n"
        "updated = dataclasses.replace(record, owner=identity)\n"
        "cli._write_active_state(config, cli._replace_record(state, updated),"
        " action='test-owner-bound', slot='slot01')\n"
        "base = dict(project_root=root, machine=None, slot='slot01',"
        " agent='codex-1', owner_pid=os.getpid(), expected_generation=1,"
        " wait_lock=0.0)\n"
        "cli._cmd_write_handoff(argparse.Namespace(**base, from_file=source))\n"
        "cli._cmd_release(argparse.Namespace(**base))\n"
    )
    env = dict(os.environ)
    py_root = str(Path(cli.__file__).resolve().parents[1])
    env["PYTHONPATH"] = py_root + os.pathsep + env.get("PYTHONPATH", "")
    proc = subprocess.run(
        ["python3", "-c", helper, str(project), str(source)],
        capture_output=True, text=True, check=False, timeout=300, env=env,
    )
    assert proc.returncode == 0, proc.stderr


@pytest.mark.ordinary_environment
def test_released_generation_rebinds_only_with_coordinator_authorization(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    _release_with_dying_owner(project, tmp_path)
    plain = transfer(project, os.getpid())
    assert plain.returncode == 3 and "was released by its owner" in plain.stderr
    assert "--reopen" in plain.stderr
    unauthorized = command(
        project, "transfer-owner", "slot01", "--agent", "codex-1",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        "--reopen", "--coordinator-pid", str(os.getpid()),
        "--reason", "lane resumed after restart",
    )
    assert unauthorized.returncode == 3, unauthorized.stderr
    assert owner_pid(project) != os.getpid()
    reopened = command(
        project, "transfer-owner", "slot01", "--agent", "codex-1",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        "--reopen", "--coordinator-authorized",
        "--coordinator-pid", str(os.getpid()),
        "--reason", "lane resumed after restart",
    )
    assert reopened.returncode == 0, reopened.stderr
    assert "reopened slot=slot01" in reopened.stdout
    assert "generation=2" in reopened.stdout
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    assert row["generation"] == 2 and row["owner"]["pid"] == os.getpid()
    event = last_active_event(project)
    assert event.get("action") == "owner-reopened", event
    evidence = event["evidence"]
    assert isinstance(evidence, dict)
    assert evidence["coordinator_authorized"] is True
    assert evidence["reason"] == "lane resumed after restart"
    assert evidence["previous_generation"] == 1


@pytest.mark.ordinary_environment
def test_the_slot_box_is_recognised_only_for_its_own_slot() -> None:
    boxed = (
        "/user.slice/user-1.slice/user@1.service/app.slice/app-wrkslots.slice/"
        "app-wrkslots-kvm\\x2d20261012.slice/wrkslots-run-kvm\\x2d20261012-1073417.scope"
    )
    assert cli._cgroup_in_slot_box(boxed, "agent", "kvm-20261012")
    assert cli._cgroup_in_slot_box("/wrkslots.slice/wrkslots-slot01.slice/x.scope", "agent", "slot01")
    for slot in ("kvm", "20261012", "kvm-2026101"):
        assert not cli._cgroup_in_slot_box(boxed, "agent", slot), slot
    assert not cli._cgroup_in_slot_box(
        "/app.slice/app-wrkslots-box-kvm\\x2d20261012.slice/a.scope", "agent", "kvm-20261012"
    )
    assert not cli._cgroup_in_slot_box("/app.slice/herdr-server.service", "agent", "kvm-20261012")
