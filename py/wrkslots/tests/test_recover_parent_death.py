"""Parent-death for the recover-absent commands (#1931).

Each command in `_PARENT_DEATH_COMMANDS` is journal-resumable, so
arming the parent-death signal is safe: these tests launch the real
command (through `wrkslots.main`, in a driver process) under a
wrapper, freeze the child once its journal is durable, kill the
wrapper, and show the child dies with it, the journal survives, and
a later rerun completes the batch. Waits poll conditions only.
"""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import time
from collections.abc import Callable
from pathlib import Path

import pytest

from wrkslots import cli as wrkslots
from wrkslots.tests.test_lifecycle import (
    absent_agent_rows_project,
    absent_row_argument,
    allow_test_host_for_absent_validate_recovery,
    create,
    make_project,
    make_validate_row_absent,
    mark_recorded_owner_dead,
    prepare_absent_agent_row,
    run_absent_agent_recovery,
    run_absent_agent_rows,
    run_absent_validate_recovery,
    write_absent_validate_input,
)

pytestmark = pytest.mark.skipif(
    sys.platform != "linux", reason="prctl parent-death signal is Linux-only"
)


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")


def _pad_event_history(project: Path, count: int = 3000) -> None:
    """Append small valid padding events so a fold takes real time."""

    config = wrkslots._load_config(str(project), "testhost")
    directory = config.control / "EVENTS.testhost"
    events = wrkslots._load_events(config, "testhost")
    sequence = len(events)
    previous = str(events[-1]["sha256"]) if events else "0" * 64
    for index in range(count):
        sequence += 1
        core: dict[str, object] = {
            "schema": wrkslots.EVENT_SCHEMA,
            "machine": "testhost",
            "sequence": sequence,
            "previous_sha256": previous,
            "recorded_at": "2026-11-19T00:00:00Z",
            "kind": "bench-padding-recorded",
            "payload": {"note": "x" * 2000, "i": index},
        }
        event = {**core, "sha256": wrkslots._event_digest(core)}
        (directory / f"{sequence:020d}.json").write_text(json.dumps(event))
        previous = str(event["sha256"])


def _wait_for(condition: Callable[[], bool], label: str) -> None:
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        if condition():
            return
        time.sleep(0.01)
    raise AssertionError(f"condition never held: {label}")


def _kill_launcher_mid_mutation(
    tmp_path: Path, project: Path, driver_args: list[str]
) -> None:
    """Run the driver under a wrapper; kill the wrapper mid-mutation.

    The child is frozen (SIGSTOP) as soon as a journal file exists, so
    the kill lands after the journal write and before completion.
    Afterwards the child must be gone (parent-death) and its journal
    must still be on disk for a later recovery.
    """

    py_dir = Path(wrkslots.__file__).resolve().parent.parent
    driver = tmp_path / "driver.py"
    driver.write_text(
        "import os, sys\n"
        f"sys.path.insert(0, {str(py_dir)!r})\n"
        "from wrkslots import cli as w\n"
        "w._short_hostname = lambda: 'testhost'\n"
        "w._absent_validate_process_snapshot = lambda: ()\n"
        "w._absent_validate_privileged_path_match = lambda _p, _t, _b=None: None\n"
        "w._user_systemd_snapshot = lambda: ()\n"
        f"sys.exit(w.main({driver_args!r}))\n"
    )
    wrapper = tmp_path / "wrapper.py"
    wrapper.write_text(
        "import subprocess, sys, time\n"
        f"p = subprocess.Popen([sys.executable, {str(driver)!r}])\n"
        "print(p.pid, flush=True)\n"
        "time.sleep(3600)\n"
    )
    config = wrkslots._load_config(str(project), "testhost")
    proc = subprocess.Popen(
        [sys.executable, str(wrapper)], stdout=subprocess.PIPE, text=True
    )
    assert proc.stdout is not None
    child_pid = int(proc.stdout.readline().strip())
    try:
        _wait_for(
            lambda: any(config.control.glob("*.journal")),
            "mutation journal written",
        )
        os.kill(child_pid, signal.SIGSTOP)
        proc.kill()
        proc.wait()
        os.kill(child_pid, signal.SIGCONT)
        _wait_for(
            lambda: not Path(f"/proc/{child_pid}").exists(),
            "child died with its launcher",
        )
        assert any(config.control.glob("*.journal")), (
            "interrupted mutation left no journal to resume"
        )
    finally:
        if proc.poll() is None:
            proc.kill()
        subprocess.run(["kill", "-9", str(child_pid)], check=False)


def test_validate_rows_die_with_launcher_then_recover_completes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, repository, _remote = make_project(tmp_path)
    records = []
    for index in range(3):
        slot = f"gone{index}"
        made = create(
            project, slot=slot, agent=f"validate-{index}", branch=None,
            slot_type="validate",
        )
        assert made.returncode == 0, made.stderr
        cur = mark_recorded_owner_dead(project, slot)
        make_validate_row_absent(project, repository, cur)
        records.append(cur)
    input_path = write_absent_validate_input(project, records)
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    _pad_event_history(project)

    _kill_launcher_mid_mutation(
        tmp_path,
        project,
        [
            "--project-root", str(project),
            "recover-absent-validate-rows",
            "--input", str(input_path),
            "--format", "json",
            "--apply", "--coordinator-authorized",
            "--coordinator-pid", str(os.getpid()),
        ],
    )
    config = wrkslots._load_config(str(project), "testhost")
    assert run_absent_validate_recovery(project, input_path, apply=True) == 0
    assert wrkslots._load_active(config).slots == ()


def test_agent_row_dies_with_launcher_then_recover_completes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, repository, _remote = make_project(tmp_path)
    record = prepare_absent_agent_row(project, repository, slot="gone-agent")
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    _pad_event_history(project)

    _kill_launcher_mid_mutation(
        tmp_path,
        project,
        [
            "--project-root", str(project),
            "recover-absent-agent-row", record.slot,
            "--expected-generation", str(record.generation),
            "--record-sha256", wrkslots._record_sha256(record),
            "--apply", "--coordinator-authorized",
            "--coordinator-pid", str(os.getpid()),
        ],
    )
    config = wrkslots._load_config(str(project), "testhost")
    assert run_absent_agent_recovery(project, record, apply=True) == 0
    assert wrkslots._load_active(config).slots == ()


def test_agent_rows_die_with_launcher_then_recover_completes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote, records = absent_agent_rows_project(
        tmp_path, monkeypatch, count=3
    )
    _pad_event_history(project)
    rows = [absent_row_argument(record) for record in records]
    driver_rows: list[str] = []
    for row in rows:
        driver_rows += ["--row", row]

    _kill_launcher_mid_mutation(
        tmp_path,
        project,
        [
            "--project-root", str(project),
            "recover-absent-agent-rows", *driver_rows,
            "--format", "json",
            "--apply", "--coordinator-authorized",
            "--coordinator-pid", str(os.getpid()),
        ],
    )
    config = wrkslots._load_config(str(project), "testhost")
    returncode, _out, _err = run_absent_agent_rows(project, *driver_rows, apply=True)
    assert returncode == 0
    assert wrkslots._load_active(config).slots == ()
