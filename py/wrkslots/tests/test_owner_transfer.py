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
    command,
    control_directory,
    create,
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
