"""A stale row elsewhere never refuses a heartbeat (#2396).

One ACTIVE row whose source repository clone was deleted used to refuse
every heartbeat on the host with "source repository ... does not exist or
cannot be resolved": every repository-requiring state load resolved every
row's repository, and the scoped storage scan raised for the stale row
while it collected Git registrations. A heartbeat now loads the other rows
lexically and resolves only its own row's repositories. The stale row is
named in the retained-storage warning, and its own heartbeat still
refuses, naming the row and its missing repository. Both editions.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.rust_binary import rust_wrkslotsd
from wrkslots.tests.test_lifecycle import (
    active_slots,
    command,
    control_directory,
    create,
    git,
    make_project,
)

AU_ROOT = Path(__file__).resolve().parents[3]


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")


def _project_with_stale_row(tmp_path: Path) -> Path:
    """slot01 from the project's repository; slot02 from a second clone, then deleted."""
    project, _repository, remote = make_project(tmp_path)
    second = project / "second"
    subprocess.run(
        ["git", "clone", str(remote), str(second)],
        check=True,
        capture_output=True,
        text=True,
    )
    git(second, "config", "user.name", "Wrkslots Test")
    git(second, "config", "user.email", "wrkslots@example.invalid")
    made = create(project, slot="slot01", agent="codex-1", branch="codex/task1")
    assert made.returncode == 0, made.stderr
    made = create(
        project, slot="slot02", agent="codex-2", branch="codex/task2", repository_name="second"
    )
    assert made.returncode == 0, made.stderr
    shutil.rmtree(second)
    return project


def _row(project: Path, slot: str) -> dict[str, object]:
    for row in active_slots(project):
        assert isinstance(row, dict)
        if row["slot"] == slot:
            return row
    raise AssertionError(f"no active row for {slot}")


def _event_count(project: Path) -> int:
    return len(list((control_directory(project) / "EVENTS.testhost").glob("*.json")))


def _heartbeat(project: Path, slot: str, agent: str) -> subprocess.CompletedProcess[str]:
    return command(
        project,
        "heartbeat",
        slot,
        "--agent",
        agent,
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
    )


def _rust_heartbeat(project: Path, slot: str, agent: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            str(rust_wrkslotsd()), "heartbeat", slot, "--agent", agent,
            "--owner-pid", str(os.getpid()), "--expected-generation", "1",
            "--root", str(project),
        ],
        cwd=AU_ROOT,
        capture_output=True,
        text=True,
        check=False,
        timeout=900,
        env=dict(os.environ, WRKSLOTSD_FIXTURE_WRITER="1"),
    )


def test_heartbeat_succeeds_beside_a_row_whose_repository_was_deleted(tmp_path: Path) -> None:
    project = _project_with_stale_row(tmp_path)
    events = _event_count(project)
    stale_before = _row(project, "slot02")

    done = _heartbeat(project, "slot01", "codex-1")

    assert done.returncode == 0, done.stderr
    assert _event_count(project) == events + 1
    renewed = _row(project, "slot01")
    assert f"heartbeat slot=slot01 generation=1 at={renewed['heartbeat_at']}" in done.stdout
    # The stale row is named, retained and untouched.
    assert "missing or inconsistent storage" in done.stderr
    assert "agent:slot02" in done.stderr
    assert _row(project, "slot02") == stale_before


def test_the_stale_rows_own_heartbeat_is_refused_by_name(tmp_path: Path) -> None:
    project = _project_with_stale_row(tmp_path)
    events = _event_count(project)
    before = _row(project, "slot02")

    done = _heartbeat(project, "slot02", "codex-2")

    assert done.returncode == 3
    assert "slot slot02: source repository 'second' does not exist or cannot be resolved" in (
        done.stderr
    )
    assert _event_count(project) == events
    assert _row(project, "slot02") == before


def test_an_unscoped_storage_scan_still_refuses_the_missing_repository(tmp_path: Path) -> None:
    """Only a scan scoped to a target tolerates another row's missing repository."""
    project = _project_with_stale_row(tmp_path)
    config = cli._load_config(str(project), None)
    states, _archives = cli._validate_global_state(config, require_repository=False)

    with pytest.raises(cli.Refusal, match="source repository 'second' does not exist"):
        cli._registry_storage_inconsistencies(config, states, verify_row_identity=False)
    scoped = cli._assert_registry_storage_consistent(config, states, target_slot="slot01")
    assert scoped.stale == ("agent:slot02",)


def test_rust_heartbeat_succeeds_beside_a_stale_row_and_refuses_it_by_name(
    tmp_path: Path,
) -> None:
    project = _project_with_stale_row(tmp_path)
    shard = control_directory(project) / "ACTIVE.testhost.json"
    before = json.loads(shard.read_text(encoding="utf-8"))
    events = _event_count(project)

    done = _rust_heartbeat(project, "slot01", "codex-1")

    assert done.returncode == 0, done.stderr
    assert _event_count(project) == events + 1
    after = json.loads(shard.read_text(encoding="utf-8"))
    assert after["revision"] == before["revision"] + 1
    rows_before = {row["slot"]: row for row in before["slots"]}
    rows_after = {row["slot"]: row for row in after["slots"]}
    assert rows_after["slot02"] == rows_before["slot02"]
    assert {key: value for key, value in rows_after["slot01"].items() if key != "heartbeat_at"} == {
        key: value for key, value in rows_before["slot01"].items() if key != "heartbeat_at"
    }

    refused = _rust_heartbeat(project, "slot02", "codex-2")

    assert refused.returncode != 0
    assert "slot slot02: source repository 'second' does not exist or cannot be resolved" in (
        refused.stderr
    )
    assert _event_count(project) == events + 1
