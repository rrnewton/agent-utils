"""One command must not re-read or re-replay a settled event-log prefix.

As in test_census_cost, every test pins a COST property and an EQUIVALENCE
property together. The costs are counts of event parses and replayed events,
never wall time; the equivalence is that the memoized answer, including every
refusal, is the answer a cold load gives.
"""

from __future__ import annotations

import dataclasses
import json
import os
import shutil
import time
from collections.abc import Sequence
from pathlib import Path
from typing import cast

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import (
    commit_validation_removal_schema,
    make_project,
    prepare_cross_repository_target,
    prepare_dead_validate_slots,
    prepare_validation_removal_proof,
    stub_validate_batch_censuses,
)


class _Work:
    """Count event parses and the events each replay folds."""

    def __init__(self, monkeypatch: pytest.MonkeyPatch) -> None:
        self.parses = 0
        self.folded: list[int] = []
        self.counting = True
        parse = cli._event_from_path
        resume = cli._resume_replay

        def counted_parse(
            path: Path,
            machine: str,
            expected_sequence: int,
            previous: str,
            *,
            require_filename: bool = True,
        ) -> dict[str, object]:
            if self.counting:
                self.parses += 1
            return parse(
                path,
                machine,
                expected_sequence,
                previous,
                require_filename=require_filename,
            )

        def counted_resume(
            config: cli.Config,
            key: tuple[cli.Config, str, bool],
            events: Sequence[dict[str, object]],
        ) -> tuple[int, cli._ReplayFold]:
            start, fold = resume(config, key, events)
            if self.counting:
                self.folded.append(len(events) - start)
            return start, fold

        monkeypatch.setattr(cli, "_event_from_path", counted_parse)
        monkeypatch.setattr(cli, "_resume_replay", counted_resume)


def _treat_every_event_as_settled(monkeypatch: pytest.MonkeyPatch) -> None:
    later = cli._EVENT_MEMO_STABLE_NS * 10
    monkeypatch.setattr(cli, "_event_memo_clock_ns", lambda: time.time_ns() + later)


def _dead_validate_batch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, count: int
) -> tuple[Path, list[str]]:
    project, repository, _remote = make_project(tmp_path)
    commit_validation_removal_schema(repository)
    slots = [f"memo-{index}" for index in range(count)]
    prepare_dead_validate_slots(project, slots)
    proofs = {slot: prepare_validation_removal_proof(project, slot) for slot in slots}
    stub_validate_batch_censuses(monkeypatch)
    argv = [
        "--project-root",
        str(project),
        "remove-validate-batch",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
        "--format",
        "json",
    ]
    for slot in slots:
        argv += ["--slot", f"{slot}=1"]
    for slot in slots:
        argv += [
            "--validation-proof-manifest",
            proofs[slot][0].relative_to(project).as_posix(),
        ]
    for slot in slots:
        argv += [
            "--completed-record",
            proofs[slot][1]["run-record"].relative_to(project).as_posix(),
        ]
    return project, argv


def _event_paths(project: Path) -> list[Path]:
    config = cli._load_config(str(project), "testhost")
    return cli._event_files(cli._event_directory(config))


def test_validate_batch_parses_each_event_once_and_folds_each_event_once(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """The control for the production deadline expiry.

    Without the memo, one four-slot batch parsed every event file on each of its
    119 whole-log loads and replayed the whole log on each of its 56 replays.
    Remove either memo and the matching equality below fails.
    """

    project, argv = _dead_validate_batch(tmp_path, monkeypatch, 4)
    _treat_every_event_as_settled(monkeypatch)
    work = _Work(monkeypatch)

    assert cli.main(argv) == 0

    report = json.loads(capsys.readouterr().out)
    assert len(report["removed"]) == 4
    assert report["retained"] == []
    events = _event_paths(project)
    assert work.parses == len(events)
    assert len(work.folded) >= 4 * 13
    assert sum(work.folded) == len(events)


def test_validate_batch_memoized_replays_equal_cold_replays(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """Every incremental replay a batch performs matches a cold replay."""

    _project, argv = _dead_validate_batch(tmp_path, monkeypatch, 2)
    _treat_every_event_as_settled(monkeypatch)
    replay = cli._states_from_events
    comparisons = 0

    def compared(
        config: cli.Config, machine: str, *, require_repository: bool = True
    ) -> tuple[cli.ActiveState, cli.ArchiveState] | None:
        nonlocal comparisons
        warm = replay(config, machine, require_repository=require_repository)
        memo = list(cli._ACTIVE_EVENT_MEMO)
        cli._ACTIVE_EVENT_MEMO.clear()
        try:
            cold = replay(config, machine, require_repository=require_repository)
        finally:
            cli._ACTIVE_EVENT_MEMO.extend(memo)
        assert warm is not None and cold is not None
        assert warm[0] == cold[0]
        assert warm[1] == cold[1]
        assert warm[1].absent_validate_recoveries == cold[1].absent_validate_recoveries
        assert warm[1].absent_agent_recoveries == cold[1].absent_agent_recoveries
        comparisons += 1
        return warm

    monkeypatch.setattr(cli, "_states_from_events", compared)

    assert cli.main(argv) == 0

    report = json.loads(capsys.readouterr().out)
    assert len(report["removed"]) == 2
    assert comparisons >= 2 * 13


def test_event_memo_keeps_only_settled_files_and_rechecks_their_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    prepare_dead_validate_slots(project, ("memo",))
    config = cli._load_config(str(project), "testhost")
    paths = _event_paths(project)
    count = len(paths)
    changed = [os.lstat(path).st_ctime_ns for path in paths]
    work = _Work(monkeypatch)
    clock = [min(changed) + cli._EVENT_MEMO_STABLE_NS - 1]
    monkeypatch.setattr(cli, "_event_memo_clock_ns", lambda: clock[0])
    cold = cli._load_event_paths(paths, config.machine)

    with cli._event_memo_scope() as memo:
        # Inside the change-time window nothing is kept, so a rewrite in the
        # same timestamp tick can never be answered from the memo.
        work.parses = 0
        assert cli._load_event_paths(paths, config.machine) == cold
        assert cli._load_event_paths(paths, config.machine) == cold
        assert work.parses == 2 * count
        assert memo.files == {}

        # One nanosecond short of the newest file settling keeps every other file.
        clock[0] = max(changed) + cli._EVENT_MEMO_STABLE_NS - 1
        recent = sum(1 for ns in changed if ns + cli._EVENT_MEMO_STABLE_NS > clock[0])
        assert recent >= 1
        work.parses = 0
        assert cli._load_event_paths(paths, config.machine) == cold
        assert cli._load_event_paths(paths, config.machine) == cold
        assert work.parses == count + recent
        assert len(memo.files) == count - recent
        memo.files.clear()

        clock[0] = max(changed) + cli._EVENT_MEMO_STABLE_NS
        work.parses = 0
        assert cli._load_event_paths(paths, config.machine) == cold
        assert work.parses == count
        assert cli._load_event_paths(paths, config.machine) == cold
        assert cli._load_event_paths(paths[:-1], config.machine) == cold[:-1]
        assert work.parses == count

        # An in-place rewrite that preserves size and mtime still moves ctime.
        target = paths[1]
        original = target.read_bytes()
        before = os.lstat(target)
        digest = cast(str, cold[1]["sha256"])
        replacement = "0" if digest[0] != "0" else "1"
        tampered = original.replace(digest.encode(), (replacement + digest[1:]).encode())
        assert len(tampered) == len(original) and tampered != original
        with target.open("r+b") as handle:
            handle.write(tampered)
        os.utime(target, ns=(before.st_atime_ns, before.st_mtime_ns))
        after = os.lstat(target)
        assert (after.st_ino, after.st_size, after.st_mtime_ns) == (
            before.st_ino,
            before.st_size,
            before.st_mtime_ns,
        )
        assert after.st_ctime_ns != before.st_ctime_ns
        with pytest.raises(cli.StateError, match="digest does not match its content"):
            cli._load_event_paths(paths, config.machine)

        with target.open("r+b") as handle:
            handle.write(original)
        assert cli._load_event_paths(paths, config.machine) == cold

        # A symlink is refused exactly as a cold load refuses it.
        kept = tmp_path / "kept-event.json"
        shutil.copy2(target, kept)
        target.unlink()
        target.symlink_to(kept)
        with pytest.raises(cli.StateError) as warm_refusal:
            cli._load_event_paths(paths, config.machine)
        memo_stack = list(cli._ACTIVE_EVENT_MEMO)
        cli._ACTIVE_EVENT_MEMO.clear()
        try:
            with pytest.raises(cli.StateError) as cold_refusal:
                cli._load_event_paths(paths, config.machine)
        finally:
            cli._ACTIVE_EVENT_MEMO.extend(memo_stack)
        assert str(warm_refusal.value) == str(cold_refusal.value)

        # A same-content replacement is a new file and is parsed again.
        target.unlink()
        shutil.copy2(kept, target)
        work.parses = 0
        assert cli._load_event_paths(paths, config.machine) == cold
        assert work.parses == 1


@pytest.mark.parametrize("require_repository", (False, True))
def test_resumed_replay_rechecks_record_paths_like_a_cold_replay(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, require_repository: bool
) -> None:
    project, repository, _remote = make_project(tmp_path)
    prepare_dead_validate_slots(project, ("memo",))
    config = cli._load_config(str(project), "testhost")
    work = _Work(monkeypatch)
    key = (config, config.machine, require_repository)

    with cli._event_memo_scope() as memo:
        first = cli._states_from_events(
            config, config.machine, require_repository=require_repository
        )
        assert first is not None
        assert key in memo.replays

        moved = repository.with_name("repo-real")
        repository.rename(moved)
        repository.symlink_to(moved, target_is_directory=True)
        with pytest.raises(cli.Refusal) as warm_refusal:
            cli._states_from_events(
                config, config.machine, require_repository=require_repository
            )
        memo_stack = list(cli._ACTIVE_EVENT_MEMO)
        cli._ACTIVE_EVENT_MEMO.clear()
        try:
            with pytest.raises(cli.Refusal) as cold_refusal:
                cli._states_from_events(
                    config, config.machine, require_repository=require_repository
                )
        finally:
            cli._ACTIVE_EVENT_MEMO.extend(memo_stack)
        assert "crosses a symlink" in str(warm_refusal.value)
        assert str(warm_refusal.value) == str(cold_refusal.value)

        repository.unlink()
        moved.rename(repository)
        work.folded.clear()
        again = cli._states_from_events(
            config, config.machine, require_repository=require_repository
        )
        assert again == first
        assert work.folded == [0]


def _warm_and_cold_refusals(
    config: cli.Config, require_repository: bool
) -> tuple[str, str]:
    """Return the refusal a memoized replay gives and the one a cold replay gives."""

    with pytest.raises(cli.Refusal) as warm:
        cli._states_from_events(
            config, config.machine, require_repository=require_repository
        )
    memo_stack = list(cli._ACTIVE_EVENT_MEMO)
    cli._ACTIVE_EVENT_MEMO.clear()
    try:
        with pytest.raises(cli.Refusal) as cold:
            cli._states_from_events(
                config, config.machine, require_repository=require_repository
            )
    finally:
        cli._ACTIVE_EVENT_MEMO.extend(memo_stack)
    return str(warm.value), str(cold.value)


def _replace_with_symlink(path: Path) -> None:
    real = path.with_name(f"{path.name}-real")
    path.rename(real)
    path.symlink_to(real, target_is_directory=True)


@pytest.mark.parametrize("require_repository", (False, True))
def test_resumed_replay_rechecks_records_that_exist_only_in_the_import(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, require_repository: bool
) -> None:
    """A production log begins with a state import that can carry many slots."""

    project, repository, _remote = make_project(tmp_path)
    prepare_dead_validate_slots(project, ("memo",))
    config = cli._load_config(str(project), "testhost")
    replayed = cli._states_from_events(config, config.machine)
    assert replayed is not None
    active, archive = replayed
    assert [record.slot for record in active.slots] == ["memo"]
    directory = cli._event_directory(config)
    for path in cli._event_files(directory):
        path.unlink()
    cli._write_event_file(
        config,
        config.machine,
        "state-imported",
        {
            "active": cli._active_to_obj(active),
            "archive": cli._archive_to_obj(archive),
            "holds": [],
        },
    )
    assert len(cli._event_files(directory)) == 1
    work = _Work(monkeypatch)

    with cli._event_memo_scope():
        assert cli._states_from_events(
            config, config.machine, require_repository=require_repository
        ) == (active, archive)
        work.folded.clear()
        assert cli._states_from_events(
            config, config.machine, require_repository=require_repository
        ) == (active, archive)
        assert work.folded == [0]
        _replace_with_symlink(repository)
        warm, cold = _warm_and_cold_refusals(config, require_repository)

    assert "crosses a symlink" in cold
    assert warm == cold


def test_required_recheck_refuses_the_first_record_in_log_order(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    prepare_dead_validate_slots(project, ("memo-a", "memo-b"))
    config = cli._load_config(str(project), "testhost")

    with cli._event_memo_scope():
        assert cli._states_from_events(config, config.machine) is not None
        for slot in ("memo-b", "memo-a"):
            _replace_with_symlink(cli._slot_directory(config, slot, "validate"))
        warm, cold = _warm_and_cold_refusals(config, True)

    assert "memo-a" in cold and "memo-b" not in cold
    assert warm == cold


def test_unrequired_recheck_refuses_the_first_repository_in_log_order(
    tmp_path: Path,
) -> None:
    project, other, _checkout = prepare_cross_repository_target(tmp_path)
    config = cli._load_config(str(project), "testhost")
    repositories = [
        checkout.repository
        for record in cli._load_active(config).slots
        for checkout in record.checkouts
    ]
    assert len(set(repositories)) == 2

    with cli._event_memo_scope():
        assert cli._states_from_events(
            config, config.machine, require_repository=False
        ) is not None
        _replace_with_symlink(other)
        _replace_with_symlink(project / "repo")
        warm, cold = _warm_and_cold_refusals(config, False)

    assert cold.endswith(f"crosses a symlink: {project / 'repo'}")
    assert warm == cold


def test_remembered_replay_is_resumed_only_on_its_own_chain(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A fold resumes only when the log still holds the chain position it folded."""

    project, _repository, _remote = make_project(tmp_path)
    prepare_dead_validate_slots(project, ("memo",))
    config = cli._load_config(str(project), "testhost")
    events = cli._load_events(config, config.machine)
    key = (config, config.machine, False)
    fold = cli._ReplayFold()

    with cli._event_memo_scope():
        cli._remember_replay(key, events[:-1], fold)
        start, resumed = cli._resume_replay(config, key, events)
        assert start == len(events) - 1
        assert resumed == fold and resumed is not fold

        # A different event at the remembered tip position is a different chain.
        forked = [*events[:-2], {**events[-2], "sha256": "0" * 64}, events[-1]]
        assert cli._resume_replay(config, key, forked)[0] == 0
        # A log shorter than the remembered prefix is never resumed.
        assert cli._resume_replay(config, key, events[:-2])[0] == 0
        # The other repository mode keeps its own fold.
        assert cli._resume_replay(config, (config, config.machine, True), events)[0] == 0

    assert cli._resume_replay(config, key, events)[0] == 0


class _ReadRecorder:
    """Record which attributes a record check reads."""

    def __init__(self, target: object, prefix: str, seen: set[str]) -> None:
        self._target = target
        self._prefix = prefix
        self._seen = seen

    def __getattr__(self, name: str) -> object:
        self._seen.add(self._prefix + name)
        value: object = getattr(self._target, name)
        if name == "checkouts":
            assert isinstance(value, tuple)
            return tuple(_ReadRecorder(item, "checkout.", self._seen) for item in value)
        return value


@pytest.mark.parametrize("require_repository", (False, True))
def test_record_path_check_key_covers_every_field_the_check_reads(
    tmp_path: Path, require_repository: bool
) -> None:
    """A resumed replay skips a duplicate check only when its inputs are equal."""

    project, _repository, _remote = make_project(tmp_path)
    prepare_dead_validate_slots(project, ("memo",))
    config = cli._load_config(str(project), "testhost")
    record = next(item for item in cli._load_active(config).slots if item.slot == "memo")
    seen: set[str] = set()

    cli._assert_record_paths(
        config,
        cast(cli.ActiveRecord, _ReadRecorder(record, "", seen)),
        require_repository=require_repository,
    )

    assert seen == {
        "slot",
        "slot_type",
        "layout",
        "checkouts",
        "checkout.name",
        "checkout.path",
        "checkout.repository",
        "checkout.remote",
        "checkout.landed_ref",
    }
    key = cli._record_path_check_key(record)
    first = record.checkouts[0]
    rest = record.checkouts[1:]
    changed_records = {
        "slot": dataclasses.replace(record, slot=f"{record.slot}-changed"),
        "slot_type": dataclasses.replace(record, slot_type=f"{record.slot_type}-changed"),
        "layout": dataclasses.replace(record, layout=f"{record.layout}-changed"),
        "checkout.name": dataclasses.replace(
            record, checkouts=(dataclasses.replace(first, name=f"{first.name}-x"), *rest)
        ),
        "checkout.path": dataclasses.replace(
            record, checkouts=(dataclasses.replace(first, path=f"{first.path}-x"), *rest)
        ),
        "checkout.repository": dataclasses.replace(
            record,
            checkouts=(dataclasses.replace(first, repository=f"{first.repository}-x"), *rest),
        ),
        "checkout.remote": dataclasses.replace(
            record, checkouts=(dataclasses.replace(first, remote=f"{first.remote}-x"), *rest)
        ),
        "checkout.landed_ref": dataclasses.replace(
            record,
            checkouts=(dataclasses.replace(first, landed_ref=f"{first.landed_ref}-x"), *rest),
        ),
    }
    assert set(changed_records) == seen - {"checkouts"}
    for name, changed in changed_records.items():
        assert cli._record_path_check_key(changed) != key, name
    # Fields the check never reads must not defeat the duplicate skip.
    unread = dataclasses.replace(
        record,
        heartbeat_at="1970-01-01T00:00:00Z",
        generation=record.generation + 1,
        checkouts=tuple(
            dataclasses.replace(checkout, head="0" * 40, branch="other")
            for checkout in record.checkouts
        ),
    )
    assert cli._record_path_check_key(unread) == key


def test_unrequired_repository_record_check_touches_storage_only_for_the_repository(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A resumed replay rechecks only repositories when paths are not required.

    That is sound only while the rest of the check never inspects storage.
    """

    project, _repository, _remote = make_project(tmp_path)
    prepare_dead_validate_slots(project, ("memo",))
    config = cli._load_config(str(project), "testhost")
    record = next(item for item in cli._load_active(config).slots if item.slot == "memo")
    references: list[str] = []

    def reference(_config: cli.Config, raw: str) -> tuple[str, Path, Path]:
        references.append(raw)
        return raw, config.root / raw, config.root

    def storage(*_args: object, **_kwargs: object) -> None:
        raise AssertionError("record path check inspected storage")

    with monkeypatch.context() as patch:
        patch.setattr(cli, "_stored_repository_reference", reference)
        for name in ("lstat", "stat", "readlink", "scandir", "listdir", "open", "access"):
            patch.setattr(os, name, storage)
        patch.setattr("builtins.open", storage)
        cli._assert_record_paths(config, record, require_repository=False)

    assert references == [checkout.repository for checkout in record.checkouts]
