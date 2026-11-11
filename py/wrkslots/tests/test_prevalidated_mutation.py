"""Single-record mutations validate the registry outside the exclusive lock (#1274 adopt-lock-narrow).

`adopt` and `create` used to run the whole-fleet validation -- every shard, the archive, the
event chains, every slot's storage -- while they held the exclusive configuration lock, the
same flock on the project root that a consumer's run-population lock takes. These tests pin
where that work runs now. They count the instrumented operations that run while the exclusive
lock is held, never wall time: the fleet-wide validation runs before the lock, and under it run
only the registry identity check, the touched record's own checks, the write, and a post-write
check that reads only the events this command wrote. Nothing under the lock loads or lists the
event log: the state the command advances and the tip it appends after come from the verified
validation, and the tip is the only earlier event read again. A writer that commits during the
lock-free validation sends the command back to validate again; after a bounded number of
overtaken rounds the command refuses as busy and writes nothing.
"""

from __future__ import annotations

import contextlib
import dataclasses
import fcntl
import os
import time
from collections.abc import Callable, Iterator, Sequence
from pathlib import Path
from typing import ParamSpec, TypeVar

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import (
    active_slots,
    checkout,
    command,
    control_directory,
    create,
    create_journal_path,
    make_project,
    slots_directory,
)

_Parameters = ParamSpec("_Parameters")
_Result = TypeVar("_Result")

#: Instrumented operations. "validate" is the fleet validation, "fleet-storage" adopt's
#: storage scan of every row, "storage-scan" the storage report (with the number of rows it
#: was given), "slot-free" create's registry-wide slot and agent check, "journals" the
#: outstanding-journal scan, "classify" create's journal classification, "event-read" one
#: event file read and hash-verified from disk rather than from the command's memo, "log-load"
#: one load of a whole event log (memoized or not), and "log-list" one listing of an event log.
_INSTRUMENTED = {
    "_validate_global_state": "validate",
    "_assert_command_registry_storage": "fleet-storage",
    "_assert_agent_and_slot_free": "slot-free",
    "_outstanding_journals": "journals",
    "_classify_create_journals": "classify",
    "_event_from_path": "event-read",
    "_load_events": "log-load",
    "_event_file_names": "log-list",
}


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")


def _exclusive_lock_held(project: Path) -> bool:
    """Whether some open file description holds the configuration lock (the project root flock).

    The probe opens its own descriptor, so it conflicts even with a lock this process holds.
    """

    fd = os.open(project, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return True
        fcntl.flock(fd, fcntl.LOCK_UN)
        return False
    finally:
        os.close(fd)


class _LockLedger:
    """Record each instrumented call and whether the exclusive lock was held during it."""

    def __init__(self, monkeypatch: pytest.MonkeyPatch, project: Path) -> None:
        self.project = project
        self.calls: list[tuple[str, bool, int | None]] = []
        self.exclusive_rounds = 0
        #: Run before each lock-free fleet validation; a concurrent writer goes here.
        self.during_validation: Callable[[], None] | None = None
        for attribute, label in _INSTRUMENTED.items():
            monkeypatch.setattr(cli, attribute, self._recorded(label, getattr(cli, attribute)))
        scan = cli._registry_storage_inconsistencies

        def recorded_scan(
            config: cli.Config,
            states: Sequence[cli.ActiveState],
            *,
            tolerate_unavailable_repositories: bool = False,
            additional_repositories: Sequence[Path] = (),
            vcs: cli._GitVcs | None = None,
            verify_row_identity: bool = True,
        ) -> tuple[cli.StorageInconsistency, ...]:
            rows = sum(len(state.slots) for state in states)
            self.calls.append(("storage-scan", _exclusive_lock_held(project), rows))
            return scan(
                config,
                states,
                tolerate_unavailable_repositories=tolerate_unavailable_repositories,
                additional_repositories=additional_repositories,
                vcs=vcs,
                verify_row_identity=verify_row_identity,
            )

        monkeypatch.setattr(cli, "_registry_storage_inconsistencies", recorded_scan)
        locks = cli._mutation_locks

        @contextlib.contextmanager
        def counted_locks(
            config: cli.Config,
            wait_seconds: float,
            *,
            deadline: float | None = None,
            shared: bool = False,
        ) -> Iterator[None]:
            with locks(config, wait_seconds, deadline=deadline, shared=shared):
                if not shared:
                    self.exclusive_rounds += 1
                yield

        monkeypatch.setattr(cli, "_mutation_locks", counted_locks)

    def _recorded(
        self, label: str, function: Callable[_Parameters, _Result]
    ) -> Callable[_Parameters, _Result]:
        def wrapper(*args: _Parameters.args, **kwargs: _Parameters.kwargs) -> _Result:
            held = _exclusive_lock_held(self.project)
            self.calls.append((label, held, None))
            if label == "validate" and not held and self.during_validation is not None:
                self.during_validation()
            return function(*args, **kwargs)

        return wrapper

    def count(self, label: str, *, held: bool) -> int:
        return sum(1 for name, was_held, _rows in self.calls if name == label and was_held == held)

    def storage_rows_under_lock(self) -> list[int | None]:
        return [rows for name, held, rows in self.calls if name == "storage-scan" and held]


def _treat_every_event_as_settled(monkeypatch: pytest.MonkeyPatch) -> None:
    """Let the event memo keep events written moments ago, as it keeps a registry's history."""

    later = cli._EVENT_MEMO_STABLE_NS * 10
    monkeypatch.setattr(cli, "_event_memo_clock_ns", lambda: time.time_ns() + later)


def _event_count(project: Path) -> int:
    return len(list((control_directory(project) / "EVENTS.testhost").glob("*.json")))


def _three_slot_project(tmp_path: Path) -> Path:
    """slot01 awaits its owner; slot02 is owned by this process and is the concurrent writer's."""

    project, _repository, _remote = make_project(tmp_path)
    made = create(project, bind_owner=False)
    assert made.returncode == 0, made.stderr
    made = create(project, slot="slot02", agent="codex-2", branch="codex/task2")
    assert made.returncode == 0, made.stderr
    return project


def _adopt_argv(project: Path) -> list[str]:
    return [
        "--project-root", str(project), "adopt", "slot01", "--agent", "codex-1",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
    ]


def _create_argv(project: Path) -> list[str]:
    pid = str(os.getpid())
    return [
        "--project-root", str(project), "create", "slot03", "--slot-type", "agent",
        "--coordinator-authorized", "--agent", "codex-3", "--task", "task-slot03",
        "--purpose", "test slot03", "--owner-pid", pid, "--coordinator-pid", pid,
        "--repo", "product=repo", "--branch", "product=codex/task3",
    ]


def _row(project: Path, slot: str) -> dict[str, object] | None:
    for row in active_slots(project):
        assert isinstance(row, dict)
        if row["slot"] == slot:
            return row
    return None


def _concurrent_hold_toggle(project: Path, writes: list[int]) -> Callable[[], None]:
    """A real writer in another process: hold slot02, then release it, then hold it again."""

    def write() -> None:
        if len(writes) % 2 == 0:
            completed = command(project, "hold", "slot02", "--reason", "concurrent writer")
        else:
            completed = command(project, "unhold", "slot02")
        assert completed.returncode == 0, completed.stderr
        writes.append(completed.returncode)

    return write


def test_adopt_runs_no_fleet_validation_while_holding_the_exclusive_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project = _three_slot_project(tmp_path)
    _treat_every_event_as_settled(monkeypatch)
    events_before = _event_count(project)
    ledger = _LockLedger(monkeypatch, project)

    assert cli.main(_adopt_argv(project)) == 0, capsys.readouterr().err

    row = _row(project, "slot01")
    assert row is not None and isinstance(row["owner"], dict)
    assert row["owner"]["pid"] == os.getpid()
    written = _event_count(project) - events_before
    assert written == 1
    # The fleet-wide work ran exactly once, and never under the exclusive lock.
    assert ledger.count("validate", held=False) == 1
    assert ledger.count("validate", held=True) == 0
    assert ledger.count("fleet-storage", held=False) == 1
    assert ledger.count("fleet-storage", held=True) == 0
    assert ledger.count("journals", held=True) == 0
    # Under the lock the only storage scan is the touched record's own.
    assert ledger.storage_rows_under_lock() == [1]
    # One exclusive acquisition. Under it the log was neither loaded nor listed, and the only
    # events read from disk were the verified tip, which the new event follows, and the event
    # the adopt itself wrote.
    assert ledger.exclusive_rounds == 1
    assert ledger.count("log-load", held=True) == 0
    assert ledger.count("log-list", held=True) == 0
    assert ledger.count("event-read", held=True) == 1 + written


def test_create_runs_no_fleet_validation_while_holding_the_exclusive_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project = _three_slot_project(tmp_path)
    _treat_every_event_as_settled(monkeypatch)
    events_before = _event_count(project)
    ledger = _LockLedger(monkeypatch, project)

    assert cli.main(_create_argv(project)) == 0, capsys.readouterr().err

    row = _row(project, "slot03")
    assert row is not None and row["agent"] == "codex-3"
    assert checkout(project, "slot03").is_dir()
    written = _event_count(project) - events_before
    assert written >= 1
    assert ledger.count("validate", held=False) == 1
    assert ledger.count("validate", held=True) == 0
    assert ledger.count("slot-free", held=False) == 1
    assert ledger.count("slot-free", held=True) == 0
    assert ledger.count("journals", held=True) == 0
    assert ledger.count("classify", held=False) + ledger.count("classify", held=True) == 0
    # The fleet storage report (both existing rows) ran lock-free; none ran under the lock.
    assert [rows for name, held, rows in ledger.calls if name == "storage-scan"] == [2]
    assert ledger.storage_rows_under_lock() == []
    assert ledger.exclusive_rounds == 1
    # The journal writes, the active-state write and the journal's completion all append
    # after the verified tip; under the lock the log was neither loaded nor listed.
    assert ledger.count("log-load", held=True) == 0
    assert ledger.count("log-list", held=True) == 0
    assert ledger.count("event-read", held=True) == 1 + written


@pytest.mark.parametrize("operation", ("adopt", "create"))
def test_a_writer_that_commits_during_validation_sends_the_command_back_to_validate(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    operation: str,
) -> None:
    project = _three_slot_project(tmp_path)
    ledger = _LockLedger(monkeypatch, project)
    writes: list[int] = []
    write = _concurrent_hold_toggle(project, writes)

    def first_round_only() -> None:
        if not writes:
            write()

    # The writer takes the exclusive locks in another process while this command validates,
    # which it can only do because the validation holds none of them.
    ledger.during_validation = first_round_only
    argv = _adopt_argv(project) if operation == "adopt" else _create_argv(project)

    rc = cli.main(argv)

    captured = capsys.readouterr()
    assert rc == 0, captured.err
    assert writes == [0]
    hold = control_directory(project) / "HOLD.8.testhost.slot02.json"
    assert hold.exists(), sorted(path.name for path in control_directory(project).iterdir())
    if operation == "adopt":
        row = _row(project, "slot01")
        assert row is not None and isinstance(row["owner"], dict)
    else:
        assert _row(project, "slot03") is not None
    # The first round's identity no longer matched under the lock, so the command released
    # it and validated again; neither validation held the lock.
    assert ledger.count("validate", held=False) == 2
    assert ledger.count("validate", held=True) == 0
    assert ledger.exclusive_rounds == 2


@pytest.mark.parametrize("operation", ("adopt", "create"))
def test_every_round_overtaken_refuses_as_busy_and_writes_nothing(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    operation: str,
) -> None:
    project = _three_slot_project(tmp_path)
    monkeypatch.setattr(cli, "REGISTRY_VALIDATION_ATTEMPTS", 3)
    ledger = _LockLedger(monkeypatch, project)
    writes: list[int] = []
    ledger.during_validation = _concurrent_hold_toggle(project, writes)
    events_before = _event_count(project)
    argv = _adopt_argv(project) if operation == "adopt" else _create_argv(project)

    rc = cli.main(argv)

    captured = capsys.readouterr()
    assert rc == 3
    assert (
        f"registry is busy: another wrkslots mutation changed it during each of the 3 "
        f"lock-free validations this {operation} ran"
    ) in captured.err
    # Callers that retry a busy lock match this phrase; the refusal is that kind of busy.
    assert "lock is busy" in captured.err
    assert f"REMEDY: retry 'wrkslots {operation}'" in captured.err
    assert len(writes) == 3
    # Only the concurrent writer's three events were written: hold, unhold, hold.
    assert _event_count(project) - events_before == 3
    if operation == "adopt":
        row = _row(project, "slot01")
        assert row is not None and row["owner"] is None
    else:
        assert _row(project, "slot03") is None
        assert not (slots_directory(project) / "slot03").exists()
        assert not create_journal_path(project, "slot03").exists()
    assert ledger.count("validate", held=False) == 3
    assert ledger.count("validate", held=True) == 0
    assert ledger.exclusive_rounds == 3


def test_an_outstanding_journal_keeps_the_locked_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """A journal needs the recovery and classification only the locked path performs."""

    project, _repository, _remote = make_project(tmp_path)
    made = create(project, bind_owner=False)
    assert made.returncode == 0, made.stderr
    interrupted = create(
        project,
        slot="slot02",
        agent="codex-2",
        branch="codex/task2",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"},
    )
    assert interrupted.returncode == 86, interrupted.stderr
    assert create_journal_path(project, "slot02").exists()
    ledger = _LockLedger(monkeypatch, project)

    # adopt refuses on the interrupted create exactly as before, from under the lock.
    assert cli.main(_adopt_argv(project)) == 3
    captured = capsys.readouterr()
    assert "interrupted mutation recorded in" in captured.err
    assert "CREATE." in captured.err
    assert ledger.count("journals", held=True) >= 1
    row = _row(project, "slot01")
    assert row is not None and row["owner"] is None

    # create beside an unrelated interrupted create still succeeds, validating under the
    # lock as it always did, because that classification needs the lock.
    ledger.calls.clear()
    assert cli.main(_create_argv(project)) == 0, capsys.readouterr().err
    assert _row(project, "slot03") is not None
    assert ledger.count("validate", held=True) >= 1
    assert ledger.count("classify", held=True) >= 1


def test_registry_identity_covers_registry_files_and_ignores_everything_else(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    config = cli._load_config(str(project), "testhost")
    before = cli._registry_identity(config)
    names = {name for name, _identity in before.entries}
    assert "ACTIVE.testhost.json" in names and "ARCHIVED.testhost.json" in names
    assert any(name.startswith("EVENTS.testhost/") for name in names)
    assert not any(name.endswith(".lock") for name in names)

    # Not registry state: lock files, storage kept beside the registry (a large sparse image
    # is listed nowhere and never read), and the files of a slot.
    for lock in control_directory(project).glob("*.lock"):
        os.utime(lock, ns=(1, 1))
    image = control_directory(project) / "project.img"
    with image.open("wb") as handle:
        handle.truncate(1 << 36)
    (checkout(project) / "scratch.txt").write_text("work\n", encoding="utf-8")
    assert cli._registry_identity(config) == before
    with image.open("r+b") as handle:
        handle.seek(1 << 30)
        handle.write(b"changed")
    assert cli._registry_identity(config) == before

    # Registry state: a journal appearing, a partial write left behind, an event appended.
    journal = control_directory(project) / "CREATE.8.testhost.6.slot09.journal"
    journal.write_text("{}\n", encoding="utf-8")
    after = cli._registry_identity(config)
    assert before.changed_names(after) == [journal.name]
    journal.unlink()
    assert cli._registry_identity(config) == before
    held = command(project, "hold", "slot01", "--reason", "identity test")
    assert held.returncode == 0, held.stderr
    changed = before.changed_names(cli._registry_identity(config))
    assert "HOLD.8.testhost.slot01.json" in changed
    assert any(name.startswith("EVENTS.testhost/") for name in changed)
    described = cli._describe_registry_change(before, cli._registry_identity(config))
    assert "HOLD.8.testhost.slot01.json" in described
    assert "file(s) in EVENTS.testhost" in described


def test_post_write_check_rederives_local_rows_and_falls_back_for_anything_else(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    for slot, agent, branch in (("slot01", "codex-1", "codex/task"), ("slot02", "codex-2", "codex/task2")):
        made = create(project, slot=slot, agent=agent, branch=branch)
        assert made.returncode == 0, made.stderr
    config = cli._load_config(str(project), "testhost")
    states, archives = cli._validate_global_state(config)
    before = cli._global_rows(states, archives)
    reference = cli._registry_identity(config)
    validations: list[str] = []
    validate = cli._validate_global_state

    def counted(
        config: cli.Config, *, require_repository: bool = True
    ) -> tuple[list[cli.ActiveState], list[cli.ArchiveState]]:
        validations.append("validate")
        return validate(config, require_repository=require_repository)

    monkeypatch.setattr(cli, "_validate_global_state", counted)
    beat = command(
        project, "heartbeat", "slot01", "--agent", "codex-1", "--owner-pid", str(os.getpid()),
        "--expected-generation", "1",
    )
    assert beat.returncode == 0, beat.stderr

    # Only this machine's shard and log changed: its rows are re-derived, nothing else read.
    cli._assert_registry_change_limited_to_slot(config, reference, before, "slot01")
    assert validations == []
    # The same cheap check still sees that a row other than the named slot changed.
    with pytest.raises(cli.StateError, match="changed an unrelated registry row"):
        cli._assert_registry_change_limited_to_slot(config, reference, before, "slot02")
    assert validations == []
    # A change outside this machine's shard and log (here a hold record) gets the full
    # re-validation the command always ran.
    held = command(project, "hold", "slot02", "--reason", "post-check test")
    assert held.returncode == 0, held.stderr
    cli._assert_registry_change_limited_to_slot(config, reference, before, "slot01")
    assert validations == ["validate"]
    # Without a reference (the locked path) the full re-validation always runs.
    cli._assert_registry_change_limited_to_slot(config, None, before, "slot01")
    assert validations == ["validate", "validate"]


def test_post_write_check_reads_only_the_events_appended_after_the_verified_tip(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    for slot, agent, branch in (("slot01", "codex-1", "codex/task"), ("slot02", "codex-2", "codex/task2")):
        made = create(project, slot=slot, agent=agent, branch=branch)
        assert made.returncode == 0, made.stderr
    config = cli._load_config(str(project), "testhost")
    states, archives = cli._validate_global_state(config)
    before = cli._global_rows(states, archives)
    reference = cli._registry_identity(config)

    # The verified tip is what loading the whole log yields: the same state and the same
    # writer position, from the last event file alone.
    local = cli._verified_local_log(config, reference, states, archives)
    assert local is not None
    assert local.sequence == _event_count(project)
    assert local.active == cli._load_active(config)
    assert local.archive == cli._load_archive(config)
    loaded = cli._event_writer(config, "testhost")
    positioned = local.writer(config)
    assert (positioned.directory, positioned.sequence, positioned.previous_sha256) == (
        loaded.directory,
        loaded.sequence,
        loaded.previous_sha256,
    )
    # Nothing validated for this machine, or no event file: the original path.
    assert cli._verified_local_log(config, reference, [], archives) is None
    empty = cli._RegistryIdentity(reference.config_sha256, ())
    assert cli._verified_local_log(config, empty, states, archives) is None

    beat = command(
        project, "heartbeat", "slot01", "--agent", "codex-1", "--owner-pid", str(os.getpid()),
        "--expected-generation", "1",
    )
    assert beat.returncode == 0, beat.stderr
    appended = _event_count(project) - local.sequence
    assert appended >= 1
    loads: list[str] = []
    reads: list[str] = []
    load = cli._load_events
    read = cli._event_from_path

    def counted_load(
        config: cli.Config, machine: str | None = None
    ) -> tuple[dict[str, object], ...]:
        loads.append(machine or config.machine)
        return load(config, machine)

    def counted_read(
        path: Path,
        machine: str,
        expected_sequence: int,
        previous: str,
        *,
        require_filename: bool = True,
    ) -> dict[str, object]:
        reads.append(path.name)
        return read(path, machine, expected_sequence, previous, require_filename=require_filename)

    monkeypatch.setattr(cli, "_load_events", counted_load)
    monkeypatch.setattr(cli, "_event_from_path", counted_read)

    # The heartbeat's events name slot01 and advance its record by one revision: reading
    # just those events is the whole check.
    cli._assert_registry_change_limited_to_slot(config, reference, before, "slot01", local)
    assert loads == []
    assert reads == [f"{sequence:020d}.json" for sequence in range(local.sequence + 1, local.sequence + appended + 1)]
    # Checked for slot02, the same run names another slot, so the log is replayed and the
    # exact check reports the changed row.
    with pytest.raises(cli.StateError, match="changed an unrelated registry row"):
        cli._assert_registry_change_limited_to_slot(config, reference, before, "slot02", local)
    assert loads == ["testhost"]
    # A run that does not start right after the verified tip is not accepted either.
    shifted = dataclasses.replace(local, sequence=local.sequence - 1)
    assert not cli._appended_events_limited_to_slot(
        config, shifted, [f"{local.sequence + 1:020d}.json"], "slot01"
    )


@pytest.mark.parametrize("operation", ("adopt", "create"))
def test_a_quarantine_only_change_during_validation_sends_the_command_back_to_validate(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    operation: str,
) -> None:
    """The quarantined journals are registry state the validation reads (review F1).

    Nothing but the quarantine directory changes between the lock-free validation and the
    identity compare, so only an identity that covers it sends the command back.
    """

    project = _three_slot_project(tmp_path)
    ledger = _LockLedger(monkeypatch, project)
    quarantine = control_directory(project) / "quarantine"
    journals: list[str] = []

    def quarantine_a_journal() -> None:
        if not journals:
            quarantine.mkdir(exist_ok=True)
            journal = quarantine / "CREATE.8.testhost.6.slot09.journal"
            journal.write_text("{}\n", encoding="utf-8")
            journals.append(journal.name)

    ledger.during_validation = quarantine_a_journal
    argv = _adopt_argv(project) if operation == "adopt" else _create_argv(project)

    rc = cli.main(argv)

    captured = capsys.readouterr()
    assert rc == 0, captured.err
    assert journals == ["CREATE.8.testhost.6.slot09.journal"]
    if operation == "adopt":
        row = _row(project, "slot01")
        assert row is not None and isinstance(row["owner"], dict)
    else:
        assert _row(project, "slot03") is not None
    assert ledger.count("validate", held=False) == 2
    assert ledger.count("validate", held=True) == 0
    assert ledger.exclusive_rounds == 2


def test_registry_identity_covers_quarantined_journals_and_slot_image_records(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    config = cli._load_config(str(project), "testhost")
    control = control_directory(project)
    quarantine = control / "quarantine"
    quarantine.mkdir()
    notes = quarantine / "README.txt"
    notes.write_text("operator notes\n", encoding="utf-8")
    image_directory = control / "slot-images" / "agent" / "slot01"
    image_directory.mkdir(parents=True)
    image = image_directory / "slot.img"
    image.write_bytes(b"\0" * 4096)
    before = cli._registry_identity(config)

    # Not registry state: another file in the quarantine directory, and the image beside a
    # record, whose times change with every write inside the slot.
    notes.write_text("operator notes, edited\n", encoding="utf-8")
    with image.open("r+b") as handle:
        handle.write(b"written")
    assert cli._registry_identity(config) == before

    # Registry state: a quarantined journal, and a slot's image record.
    journal = quarantine / "CREATE.8.testhost.6.slot09.journal"
    journal.write_text("{}\n", encoding="utf-8")
    quarantined = cli._registry_identity(config)
    assert before.changed_names(quarantined) == [f"quarantine/{journal.name}"]
    assert "1 file(s) in quarantine" in cli._describe_registry_change(before, quarantined)
    record = image_directory / "IMAGE.json"
    record.write_text("{}\n", encoding="utf-8")
    recorded = cli._registry_identity(config)
    assert quarantined.changed_names(recorded) == ["slot-images/agent/slot01/IMAGE.json"]
    replacement = image_directory / "IMAGE.json.tmp"
    replacement.write_text('{"replaced": true}\n', encoding="utf-8")
    os.replace(replacement, record)
    assert recorded.changed_names(cli._registry_identity(config)) == [
        "slot-images/agent/slot01/IMAGE.json"
    ]


def test_post_write_check_accepts_only_the_slots_own_image_record(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    for slot, agent, branch in (("slot01", "codex-1", "codex/task"), ("slot02", "codex-2", "codex/task2")):
        made = create(project, slot=slot, agent=agent, branch=branch)
        assert made.returncode == 0, made.stderr
    config = cli._load_config(str(project), "testhost")
    states, archives = cli._validate_global_state(config)
    before = cli._global_rows(states, archives)
    reference = cli._registry_identity(config)
    local = cli._verified_local_log(config, reference, states, archives)
    assert local is not None
    validations: list[str] = []
    validate = cli._validate_global_state

    def counted(
        config: cli.Config, *, require_repository: bool = True
    ) -> tuple[list[cli.ActiveState], list[cli.ArchiveState]]:
        validations.append("validate")
        return validate(config, require_repository=require_repository)

    monkeypatch.setattr(cli, "_validate_global_state", counted)
    images = control_directory(project) / "slot-images" / "agent"

    # An image-backed create writes its own slot's record: still the cheap check.
    own = images / "slot01" / "IMAGE.json"
    own.parent.mkdir(parents=True)
    own.write_text("{}\n", encoding="utf-8")
    cli._assert_registry_change_limited_to_slot(config, reference, before, "slot01", local)
    assert validations == []
    # Another slot's record is not this mutation's to write: the full re-validation runs.
    other = images / "slot02" / "IMAGE.json"
    other.parent.mkdir(parents=True)
    other.write_text("{}\n", encoding="utf-8")
    cli._assert_registry_change_limited_to_slot(config, reference, before, "slot01", local)
    assert validations == ["validate"]
