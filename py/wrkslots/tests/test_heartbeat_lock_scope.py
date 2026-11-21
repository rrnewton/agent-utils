"""heartbeat validates lock-free and holds the exclusive lock only to write (#1808-A).

`heartbeat` used to run the whole-fleet validation twice plus the registry
storage scan while it held the exclusive configuration lock, so dozens of
heartbeating lanes starved every other mutation. These tests pin where that
work runs now, by counting instrumented operations while the exclusive lock
is held, never by wall time: the fleet-wide work runs before the lock, and
under it run only the registry identity check, the touched record's own
checks, the write, and a post-write check that reads only the appended
events. A writer that commits during the lock-free validation sends the
command back to validate; after a bounded number of overtaken rounds the
command refuses as busy and writes nothing.
"""

from __future__ import annotations

import contextlib
import fcntl
import os
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import ParamSpec, TypeVar

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import (
    active_slots,
    command,
    control_directory,
    create,
    make_project,
)

_Parameters = ParamSpec("_Parameters")
_Result = TypeVar("_Result")

_INSTRUMENTED = {
    "_validate_global_state": "validate",
    "_registry_storage_inconsistencies": "storage-scan",
    "_load_events": "log-load",
    "_event_file_names": "log-list",
}


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")
    monkeypatch.setenv("WRKSLOTS_LOCK_ORDER_CHECK", "1")


def _flock_held(path: Path, flags: int) -> bool:
    fd = os.open(path, flags | os.O_CLOEXEC)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return True
        fcntl.flock(fd, fcntl.LOCK_UN)
        return False
    finally:
        os.close(fd)


def _project_root_flock_held(project: Path) -> bool:
    """The configuration lock's legacy half: the exclusive flock on the project root."""
    return _flock_held(project, os.O_RDONLY | os.O_DIRECTORY)


def _exclusive_lock_held(project: Path) -> bool:
    """Is a mutation's exclusive lock held: the project-root flock, or the ACTIVE
    registry write lock that a heartbeat now commits under without it (#2000)?"""
    registry = control_directory(project) / "ACTIVE.global.lock"
    return _project_root_flock_held(project) or (
        registry.exists() and _flock_held(registry, os.O_RDONLY)
    )


class _LockLedger:
    def __init__(self, monkeypatch: pytest.MonkeyPatch, project: Path) -> None:
        self.project = project
        self.calls: list[tuple[str, bool]] = []
        self.exclusive_rounds = 0
        self.during_validation: Callable[[], None] | None = None
        for attribute, label in _INSTRUMENTED.items():
            monkeypatch.setattr(cli, attribute, self._recorded(label, getattr(cli, attribute)))
        locks = cli._mutation_locks

        @contextlib.contextmanager
        def counted_locks(
            config: cli.Config,
            wait_seconds: float,
            *,
            deadline: float | None = None,
            shared: bool = False,
            release_config_after_check: bool = False,
        ) -> Iterator[None]:
            with locks(
                config,
                wait_seconds,
                deadline=deadline,
                shared=shared,
                release_config_after_check=release_config_after_check,
            ):
                if not shared:
                    self.exclusive_rounds += 1
                yield

        monkeypatch.setattr(cli, "_mutation_locks", counted_locks)

    def _recorded(
        self, label: str, function: Callable[_Parameters, _Result]
    ) -> Callable[_Parameters, _Result]:
        def wrapper(*args: _Parameters.args, **kwargs: _Parameters.kwargs) -> _Result:
            held = _exclusive_lock_held(self.project)
            self.calls.append((label, held))
            if label == "validate" and not held and self.during_validation is not None:
                self.during_validation()
            return function(*args, **kwargs)

        return wrapper

    def count(self, label: str, *, held: bool) -> int:
        return sum(1 for name, was_held in self.calls if name == label and was_held == held)


def _project_with_slots(tmp_path: Path, count: int = 3) -> Path:
    project, _repository, _remote = make_project(tmp_path)
    for index in range(1, count + 1):
        made = create(
            project,
            slot=f"slot{index:02d}",
            agent=f"codex-{index}",
            branch=f"codex/task{index}",
        )
        assert made.returncode == 0, made.stderr
    return project


def _heartbeat_argv(project: Path, slot: str = "slot01", agent: str = "codex-1") -> list[str]:
    return [
        "--project-root", str(project), "heartbeat", slot,
        "--agent", agent, "--owner-pid", str(os.getpid()),
        "--expected-generation", "1",
    ]


def _event_count(project: Path) -> int:
    return len(list((control_directory(project) / "EVENTS.testhost").glob("*.json")))


def _row(project: Path, slot: str) -> dict[str, object]:
    for row in active_slots(project):
        assert isinstance(row, dict)
        if row["slot"] == slot:
            return row
    raise AssertionError(f"no row for {slot}")


def test_heartbeat_runs_no_fleet_validation_while_holding_the_exclusive_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project = _project_with_slots(tmp_path)
    events_before = _event_count(project)
    ledger = _LockLedger(monkeypatch, project)

    assert cli.main(_heartbeat_argv(project)) == 0, capsys.readouterr().err

    assert _event_count(project) - events_before == 1
    assert _row(project, "slot01")["heartbeat_at"]
    # The fleet-wide work ran before the lock, never under it.
    assert ledger.count("validate", held=False) >= 1
    assert ledger.count("validate", held=True) == 0
    assert ledger.count("storage-scan", held=True) == 0
    # Nothing under the lock loads or lists the event log.
    assert ledger.count("log-load", held=True) == 0
    assert ledger.count("log-list", held=True) == 0
    assert ledger.exclusive_rounds == 1


def test_heartbeat_revalidates_when_a_writer_commits_during_validation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project = _project_with_slots(tmp_path)
    ledger = _LockLedger(monkeypatch, project)
    writes: list[int] = []

    def concurrent_hold() -> None:
        if writes:
            return
        completed = command(project, "hold", "slot02", "--reason", "concurrent writer")
        assert completed.returncode == 0, completed.stderr
        writes.append(completed.returncode)

    ledger.during_validation = concurrent_hold

    assert cli.main(_heartbeat_argv(project)) == 0, capsys.readouterr().err

    # The concurrent hold committed once, the heartbeat revalidated, then wrote.
    assert writes == [0]
    assert ledger.count("validate", held=False) == 2
    assert ledger.count("validate", held=True) == 0


def test_heartbeat_revalidates_when_its_own_slot_changes_during_validation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """A heartbeat of the target slot itself is never tolerated (#1808 review)."""
    project = _project_with_slots(tmp_path)
    ledger = _LockLedger(monkeypatch, project)
    writes: list[int] = []

    def own_heartbeat() -> None:
        if writes:
            return
        completed = command(
            project,
            "heartbeat",
            "slot01",
            "--agent",
            "codex-1",
            "--owner-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        )
        assert completed.returncode == 0, completed.stderr
        writes.append(completed.returncode)

    ledger.during_validation = own_heartbeat

    assert cli.main(_heartbeat_argv(project)) == 0, capsys.readouterr().err
    assert writes == [0]
    assert ledger.count("validate", held=False) == 2


def test_heartbeat_refuses_when_its_slot_is_released_during_validation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """A release landing in the prepare window still refuses (#1808 review)."""
    project = _project_with_slots(tmp_path)
    notes = tmp_path / "handoff.md"
    notes.write_text("done\n", encoding="utf-8")
    handed = command(
        project,
        "write-handoff",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
        str(notes),
    )
    assert handed.returncode == 0, handed.stderr
    ledger = _LockLedger(monkeypatch, project)
    writes: list[int] = []

    def release() -> None:
        if writes:
            return
        completed = command(
            project,
            "release",
            "slot01",
            "--agent",
            "codex-1",
            "--owner-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        )
        assert completed.returncode == 0, completed.stderr
        writes.append(completed.returncode)

    ledger.during_validation = release

    rc = cli.main(_heartbeat_argv(project))

    captured = capsys.readouterr()
    assert rc == 3
    assert "was released by its owner" in captured.err


def test_heartbeat_never_repairs_an_outstanding_journal_without_the_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """An interrupted create's journal is classified under the lock or not at all."""
    project = _project_with_slots(tmp_path)
    interrupted = create(
        project,
        slot="slot09",
        agent="codex-9",
        branch="codex/task9",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"},
    )
    assert interrupted.returncode == 86, interrupted.stderr
    journals = list(control_directory(project).glob("CREATE.*.journal"))
    assert len(journals) == 1
    journal_bytes = journals[0].read_bytes()
    events_before = _event_count(project)

    rc = cli.main(_heartbeat_argv(project))

    captured = capsys.readouterr()
    # The locked path judges the journal exactly as the old code did, and
    # nothing was repaired or appended on the lock-free path.
    assert rc == 3, captured.err
    assert journals[0].read_bytes() == journal_bytes
    assert _event_count(project) == events_before


def test_heartbeat_for_a_machine_without_active_state_refuses_cleanly(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """No ACTIVE file for the machine: exit 3 refusal, never a traceback."""
    project = _project_with_slots(tmp_path)

    rc = cli.main(
        [
            "--project-root", str(project), "--machine", "otherhost",
            "heartbeat", "slot01", "--agent", "codex-1",
            "--owner-pid", str(os.getpid()), "--expected-generation", "1",
        ]
    )

    captured = capsys.readouterr()
    assert rc == 3
    assert "missing active state" in captured.err
    assert "Traceback" not in captured.err


def test_heartbeat_at_many_rows_still_counts_zero_fleet_work_under_the_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project = _project_with_slots(tmp_path, count=12)
    ledger = _LockLedger(monkeypatch, project)

    assert cli.main(_heartbeat_argv(project)) == 0, capsys.readouterr().err

    assert ledger.count("validate", held=True) == 0
    assert ledger.count("storage-scan", held=True) == 0
    assert ledger.count("log-load", held=True) == 0
    assert ledger.count("log-list", held=True) == 0
