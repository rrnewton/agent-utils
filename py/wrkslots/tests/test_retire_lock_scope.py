"""retire-pending validates candidates outside the exclusive lock (#1287 retire-lock-narrow).

`retire-pending --limit 8` held the exclusive flock on the project root
continuously for 9+ minutes on a measured host while it removed 0 slots:
each candidate's ordinary removal ran the fleet-wide validation, the
event-log loads, the storage scan, the process-use census and the salvage
derivation while it held the mutation locks, and a refusal was then
classified under the locks again. These tests pin where that work runs
now, by counting instrumented operations while the exclusive lock is
held, never by wall time (the owner's counted-evidence rule, as in
#1276): every candidate is validated and planned with no lock held, the
lock is taken per slot only to re-check that slot's identity and commit
its record or journal change, and the slow work (`git worktree remove`,
storage deletion, fence/use checks, salvage) runs outside the flock. A
concurrent non-blocking flock attempt therefore succeeds between slots,
and a batch that removes nothing never takes the exclusive lock at all.
"""

from __future__ import annotations

import contextlib
import fcntl
import json
import os
import subprocess
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import ParamSpec, TypeVar, cast

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import (
    checkout,
    commit_task,
    create,
    finish,
    make_project,
    mark_owner_dead,
    raw_command,
    set_liveness,
)

_Parameters = ParamSpec("_Parameters")
_Result = TypeVar("_Result")

#: Instrumented operations. "validate" is the fleet validation,
#: "log-load" one load of a whole event log, "storage-scan" the registry
#: storage report, "slot-unused" the process-use census / fence check,
#: "salvage" the salvage derivation, "finish-paths" the fenced deletion
#: (`git worktree remove` plus storage deletion), and "state-home" the
#: slot-state-home reclaim preparation.
_INSTRUMENTED = {
    "_validate_global_state": "validate",
    "_load_events": "log-load",
    "_registry_storage_inconsistencies": "storage-scan",
    "_assert_slot_unused": "slot-unused",
    "_salvage_agent_slot": "salvage",
    "_finish_remove_paths": "finish-paths",
    "_prepare_state_home_reclaim": "state-home",
}

#: Operations that must never run while the root flock is held.
_NEVER_HELD = ("validate", "storage-scan", "slot-unused", "salvage", "finish-paths", "state-home")


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")


def _exclusive_lock_held(project: Path) -> bool:
    """Whether some open file description holds the configuration lock.

    The probe opens its own descriptor, so it conflicts even with a lock
    this process holds.
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
    """Record each instrumented call and whether the exclusive lock was held."""

    def __init__(self, monkeypatch: pytest.MonkeyPatch, project: Path) -> None:
        self.project = project
        self.calls: list[tuple[str, bool]] = []
        self.exclusive_rounds = 0
        self.pregather_probes: list[bool] = []
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
        ) -> Iterator[None]:
            with locks(config, wait_seconds, deadline=deadline, shared=shared):
                if not shared:
                    self.exclusive_rounds += 1
                yield

        monkeypatch.setattr(cli, "_mutation_locks", counted_locks)
        pregather = cli._pregather_salvage_facts

        def recorded_pregather(*args: object, **kwargs: object) -> None:
            self.pregather_probes.append(_exclusive_lock_held(project))
            pregather(*args, **kwargs)  # type: ignore[arg-type]

        monkeypatch.setattr(cli, "_pregather_salvage_facts", recorded_pregather)

    def _recorded(
        self, label: str, function: Callable[_Parameters, _Result]
    ) -> Callable[_Parameters, _Result]:
        def wrapper(*args: _Parameters.args, **kwargs: _Parameters.kwargs) -> _Result:
            self.calls.append((label, _exclusive_lock_held(self.project)))
            return function(*args, **kwargs)

        return wrapper

    def count(self, label: str, *, held: bool) -> int:
        return sum(1 for name, was_held in self.calls if name == label and was_held == held)


def _retireable_slot(project: Path, repository: Path, tmp_path: Path, slot: str, branch: str) -> None:
    agent = f"agent-{slot}"
    created = create(project, slot=slot, agent=agent, branch=branch)
    assert created.returncode == 0, created.stderr
    commit_task(repository, checkout(project, slot), branch)
    source = tmp_path / f"handoff-{slot}.md"
    source.write_text("retire safely\n", encoding="utf-8")
    assert raw_command(
        project,
        "write-handoff",
        slot,
        "--agent",
        agent,
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--from-file",
        str(source),
    ).returncode == 0
    assert finish(project, slot=slot, agent=agent).returncode == 0
    assert raw_command(
        project, "read-handoff", slot, "--coordinator-pid", str(os.getpid())
    ).returncode == 0


def _two_retireable_slots(tmp_path: Path) -> Path:
    project, repository, _remote = make_project(tmp_path)
    _retireable_slot(project, repository, tmp_path, "slot01", "codex/task")
    _retireable_slot(project, repository, tmp_path, "slot02", "codex/task2")
    mark_owner_dead(project, slot="slot01")
    mark_owner_dead(project, slot="slot02")
    set_liveness(project, "dead")
    return project


def _retire_pending(
    project: Path, capsys: pytest.CaptureFixture[str], *, limit: int = 2
) -> dict[str, object]:
    code = cli.main(
        [
            "--project-root",
            str(project),
            "retire-pending",
            "--limit",
            str(limit),
            "--coordinator-pid",
            str(os.getpid()),
            "--format",
            "json",
        ]
    )
    assert code == 0
    payload: dict[str, object] = json.loads(capsys.readouterr().out)
    return payload


def test_retire_pending_removes_without_candidate_validation_under_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project = _two_retireable_slots(tmp_path)
    ledger = _LockLedger(monkeypatch, project)

    payload = _retire_pending(project, capsys)

    removed_slots = [
        item["slot"] for item in cast(list[dict[str, object]], payload["removed"])
    ]
    if removed_slots != ["slot01", "slot02"]:
        raise AssertionError(json.dumps(payload, indent=2))
    assert payload["retained"] == []
    # No candidate validation, census, salvage or deletion under the flock.
    for label in _NEVER_HELD:
        assert ledger.count(label, held=True) == 0, (label, ledger.calls)
    # The work still happened, with the lock released.
    for label in ("validate", "slot-unused", "salvage", "finish-paths"):
        assert ledger.count(label, held=False) > 0, (label, ledger.calls)
    # Event-log loads under the lock belong only to a slot's own commit
    # rounds (its attempt append, its basis and journal writes, and its
    # completion), never to the fleet validation: "validate" above is the
    # counted proof, and these loads read only this slot's memoized chain.
    # The flock is released between slots: each slot's lock-free gather
    # probes it free, so a concurrent non-blocking attempt succeeds there.
    assert ledger.pregather_probes == [False, False]
    assert not checkout(project, "slot01").exists()
    assert not checkout(project, "slot02").exists()


def test_retire_pending_zero_removal_validates_nothing_under_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project = _two_retireable_slots(tmp_path)
    ledger = _LockLedger(monkeypatch, project)

    def refusing_salvage(*args: object, **kwargs: object) -> tuple[dict[str, object], ...]:
        ledger.calls.append(("salvage", _exclusive_lock_held(project)))
        raise cli.Refusal("salvage refused for this test")

    monkeypatch.setattr(cli, "_salvage_agent_slot", refusing_salvage)

    payload = _retire_pending(project, capsys)

    # The measured incident's shape: a batch that removes nothing. Every
    # candidate was validated with no lock held; the only exclusive
    # rounds are the record-sized attempt appends (one per candidate),
    # and no validation ran under any of them.
    assert payload["removed"] == []
    retained = cast(list[dict[str, object]], payload["retained"])
    assert [item["slot"] for item in retained] == ["slot01", "slot02"]
    assert all("salvage refused for this test" in str(item["reason"]) for item in retained)
    assert ledger.exclusive_rounds == 2, ledger.calls
    # No validation ran under those rounds; the only loads they make are
    # this slot's own memoized event chain, for the attempt append.
    for label in _NEVER_HELD:
        assert ledger.count(label, held=True) == 0, (label, ledger.calls)
    assert checkout(project, "slot01").exists()
    assert checkout(project, "slot02").exists()


def test_retire_pending_defers_slot_changed_after_lock_free_prepare(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project = _two_retireable_slots(tmp_path)
    ledger = _LockLedger(monkeypatch, project)
    fingerprint = cli._lock_scope_fingerprint
    mutated: list[str] = []

    def mutating_fingerprint(
        config: cli.Config, record: cli.ActiveRecord, vcs: cli._GitVcs
    ) -> cli._LockScopeFingerprint:
        result = fingerprint(config, record, vcs)
        if record.slot == "slot01" and not mutated:
            mutated.append(record.slot)
            worktree = checkout(project, "slot01")
            (worktree / "late.txt").write_text("late work\n", encoding="utf-8")
            subprocess.run(
                ["git", "-C", str(worktree), "add", "late.txt"],
                check=True,
                capture_output=True,
            )
            subprocess.run(
                ["git", "-C", str(worktree), "commit", "-qm", "late work"],
                check=True,
                capture_output=True,
            )
        return result

    monkeypatch.setattr(cli, "_lock_scope_fingerprint", mutating_fingerprint)

    payload = _retire_pending(project, capsys)

    # slot01 changed between its lock-free validation and the locked
    # re-check: it is refused by name as deferred, nothing of it is
    # removed, and the batch goes on to slot02.
    assert mutated == ["slot01"]
    assert [
        item["slot"] for item in cast(list[dict[str, object]], payload["removed"])
    ] == ["slot02"]
    deferred = [
        item
        for item in cast(list[dict[str, object]], payload["deferred"])
        if item["slot"] == "slot01"
    ]
    assert deferred and "slot01" in str(deferred[0]["reason"])
    assert checkout(project, "slot01").exists()
    assert not checkout(project, "slot02").exists()
    for label in _NEVER_HELD:
        assert ledger.count(label, held=True) == 0, (label, ledger.calls)
