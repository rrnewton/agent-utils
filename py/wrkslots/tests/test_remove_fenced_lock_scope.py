"""A fenced remove deletes without the registry lock (#1735).

The private validation finish used to run its fenced deletion
(``_finish_remove_paths``) inside the completion ``_mutation_locks``
hold, so one slow tree deletion blocked every other slot's create for
as long as the deletion took. It now revalidates under the lock,
deletes with the lock released, and re-takes the lock only to record
completion -- the deferred shape ordinary removals already used.

The proof is condition-based: the deletion stub blocks on an event
the test controls, and a create on another slot must complete while
that event is still unset. No elapsed-time budget decides the result;
timeouts below only keep a broken run from hanging forever.
"""

from __future__ import annotations

import os
import threading
from pathlib import Path

import pytest

from wrkslots import cli as wrkslots
from wrkslots.tests.test_lifecycle import (
    active_slots,
    command,
    control_directory,
    create,
    dead_agent_batch_project,
    finish,
    mark_owner_dead,
    make_project,
    prepare_removal_journal,
    remove_completed_validation,
    set_liveness,
    stub_validate_batch_censuses,
)


def _remove_agent_in_process(project: Path) -> int:
    """Remove slot01 in this process, coordinated by this process."""

    return wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "slot01",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )


def _agent_project_removing(tmp_path: Path) -> Path:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    handed_off = finish(project)
    assert handed_off.returncode == 0, handed_off.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    return project


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")


def test_create_completes_while_private_remove_deletion_is_blocked(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot="target", slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    stub_validate_batch_censuses(monkeypatch)
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_a, **_k: None)

    entered = threading.Event()
    release = threading.Event()
    real_delete = wrkslots._finish_remove_paths

    def blocking_delete(*args: object, **kwargs: object) -> object:
        entered.set()
        # Hang guard only: the assertions below are event-driven.
        assert release.wait(timeout=300)
        return real_delete(*args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(wrkslots, "_finish_remove_paths", blocking_delete)

    results: list[int] = []
    remover = threading.Thread(
        target=lambda: results.append(remove_completed_validation(project))
    )
    remover.start()
    try:
        assert entered.wait(timeout=120), "remove never reached its deletion"
        # The deletion is blocked on `release`; a create on another
        # slot completes only if no registry lock is held across it.
        created = create(project, slot="next", slot_type="validate", branch=None)
        assert created.returncode == 0, created.stderr
        assert remover.is_alive(), "remove finished before its deletion was released"
    finally:
        release.set()
        remover.join(timeout=120)

    assert results == [0]
    assert [row["slot"] for row in active_slots(project) if isinstance(row, dict)] == ["next"]


def test_recover_refuses_while_the_recorded_remover_is_alive(
    tmp_path: Path,
) -> None:
    """Recover never resumes a finish whose remover still runs (#1735).

    The journal's recorded owner is rewritten to a live sleeper, as a
    remover still deleting its fenced tree would be; recover reports
    busy (exit 75) naming it and leaves journal and tree untouched.
    """

    import json
    import subprocess
    import sys

    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    journal = prepare_removal_journal(project)
    config = wrkslots._load_config(str(project), "testhost")
    raw = json.loads(journal.read_text(encoding="utf-8"))
    sleeper = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(120)"])
    try:
        identity = wrkslots._read_process_identity(sleeper.pid)
        raw["owner"] = {
            "pid": identity.pid,
            "start_ticks": identity.start_ticks,
            "boot_id": identity.boot_id,
            "host_id": identity.host_id,
            "cgroup_path": identity.cgroup_path,
        }
        wrkslots._write_journal(config, raw)
        sealed = journal.read_bytes()

        result = wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        assert result == 75
        assert journal.read_bytes() == sealed
    finally:
        sleeper.kill()
        sleeper.wait()


def test_coordinator_remover_blocked_mid_deletion_refuses_child_recover(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A real coordinator-process remover mid-deletion blocks recover (#1735).

    The remover runs in this process (its coordinator is this process)
    and is paused inside the fenced deletion. A child recover naming
    that coordinator must refuse busy (75): the deletion flock is held
    from inside the registry lock until completion is recorded, so a
    live coordinator alone never lets recover resume beside it.
    """

    project = _agent_project_removing(tmp_path)
    entered = threading.Event()
    release = threading.Event()
    real_delete = wrkslots._finish_remove_paths

    def blocking_delete(*args: object, **kwargs: object) -> object:
        entered.set()
        assert release.wait(timeout=300)
        return real_delete(*args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(wrkslots, "_finish_remove_paths", blocking_delete)
    results: list[int] = []
    remover = threading.Thread(
        target=lambda: results.append(_remove_agent_in_process(project))
    )
    remover.start()
    try:
        assert entered.wait(timeout=120), "remove never reached its deletion"
        result = command(project, "recover", "--coordinator-pid", str(os.getpid()))
        assert result.returncode == 75, result.stderr
    finally:
        release.set()
        remover.join(timeout=120)
    assert results == [0]
    lock_file = control_directory(project) / "DELETION-LOCK.ACTIVE.testhost.journal.lock"
    assert not lock_file.exists(), "deletion-lock file was not cleaned up"


def test_coordinator_remover_paused_before_completion_refuses_child_recover(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The window after deletion, before completion, is flocked too (#1735).

    The remover is paused at completion entry, after the fenced tree
    was deleted: the deletion flock is still held, so a child recover
    refuses busy instead of finishing the slot beside it.
    """

    project = _agent_project_removing(tmp_path)
    entered = threading.Event()
    release = threading.Event()
    real_delete = wrkslots._delete_prepared_deferred_finish

    def blocking_delete(*args: object, **kwargs: object) -> object:
        deleted = real_delete(*args, **kwargs)  # type: ignore[arg-type]
        entered.set()
        assert release.wait(timeout=300)
        return deleted

    monkeypatch.setattr(wrkslots, "_delete_prepared_deferred_finish", blocking_delete)
    results: list[int] = []
    remover = threading.Thread(
        target=lambda: results.append(_remove_agent_in_process(project))
    )
    remover.start()
    try:
        assert entered.wait(timeout=120), "remove never finished its deletion"
        result = command(project, "recover", "--coordinator-pid", str(os.getpid()))
        assert result.returncode == 75, result.stderr
    finally:
        release.set()
        remover.join(timeout=120)
    assert results == [0]


def test_grouped_member_paused_in_group_scan_refuses_child_recover(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A grouped member is flocked from fencing through its scan (#1735).

    While the batch waits in the group scan, each member's finish
    journal is already written and its deletion flock held, so a
    child recover of one member refuses busy.
    """

    from wrkslots.tests.test_lifecycle import in_process_agent_batch

    project, _repository, slots = dead_agent_batch_project(tmp_path, count=2)
    entered = threading.Event()
    release = threading.Event()
    real_scan = wrkslots._agent_batch_group_fenced_uses

    def blocking_scan(*args: object, **kwargs: object) -> object:
        entered.set()
        assert release.wait(timeout=300)
        return real_scan(*args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(wrkslots, "_agent_batch_group_fenced_uses", blocking_scan)
    outcomes: list[tuple[int, dict[str, object], str]] = []
    batch = threading.Thread(
        target=lambda: outcomes.append(in_process_agent_batch(project, slots))
    )
    batch.start()
    try:
        assert entered.wait(timeout=120), "batch never reached its group scan"
        result = command(
            project,
            "recover",
            "--slot",
            "slot01",
            "--coordinator-pid",
            str(os.getpid()),
        )
        assert result.returncode == 75, result.stderr
    finally:
        release.set()
        batch.join(timeout=120)
    assert outcomes[0][0] == 0


def test_refused_recover_leaves_sealed_fence_and_state_unchanged(
    tmp_path: Path,
) -> None:
    """A busy recover changes nothing before refusing (#1735).

    The finish journal is a sealed grouped member's: on the old code
    recover restored the fence mode (unsealed it) before its busy
    guard ran, so a refused recover still weakened the seal. The
    guard now runs before any side effect, so the fence stays sealed
    (mode 0), and the journal and active state stay byte-identical.
    """

    import json
    import stat
    import subprocess
    import sys

    project, _repository, slots = dead_agent_batch_project(tmp_path, count=2)
    interrupted = command(
        project,
        "remove-agent-batch",
        "--coordinator-pid",
        str(os.getpid()),
        *(f"--slot={slot}=1" for slot in slots),
        "--format",
        "json",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-agent-batch-group-seal"},
    )
    assert interrupted.returncode == 86, interrupted.stderr
    config = wrkslots._load_config(str(project), "testhost")
    scoped = wrkslots._finish_journal_path(config, "slot01")
    assert scoped.is_file()
    raw = json.loads(scoped.read_text(encoding="utf-8"))
    record = wrkslots._record_from_obj(raw["record"], "finish journal.record")
    fenced = wrkslots._finish_fenced_slot(config, record, raw)
    assert stat.S_IMODE(os.lstat(fenced).st_mode) == 0
    sleeper = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(120)"])
    try:
        identity = wrkslots._read_process_identity(sleeper.pid)
        raw["owner"] = {
            "pid": identity.pid,
            "start_ticks": identity.start_ticks,
            "boot_id": identity.boot_id,
            "host_id": identity.host_id,
            "cgroup_path": identity.cgroup_path,
        }
        wrkslots._write_journal(config, raw, journal_path=scoped)
        state_path = control_directory(project) / "ACTIVE.testhost.json"
        state_bytes = state_path.read_bytes()
        sealed = scoped.read_bytes()

        result = wrkslots.main(
            [
                "--project-root",
                str(project),
                "recover",
                "--slot",
                "slot01",
                "--coordinator-pid",
                str(os.getpid()),
            ]
        )
        assert result == 75
        assert stat.S_IMODE(os.lstat(fenced).st_mode) == 0
        assert scoped.read_bytes() == sealed
        assert state_path.read_bytes() == state_bytes
    finally:
        sleeper.kill()
        sleeper.wait()
        if fenced.exists():
            os.chmod(fenced, 0o755)


def test_grouped_batch_members_still_defer_their_deletion(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """Grouped remove-agent-batch members keep the deferred shape.

    Guard, not a regression test: this also passes on the old code.
    """

    import json as _json

    project, _repository, slots = dead_agent_batch_project(tmp_path, count=2)
    code = wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove-agent-batch",
            "--coordinator-pid",
            str(os.getpid()),
            *(f"--slot={slot}=1" for slot in slots),
            "--format",
            "json",
        ]
    )
    assert code == 0, capsys.readouterr().err
    payload = _json.loads(capsys.readouterr().out)
    assert [item["slot"] for item in payload["removed"]] == slots
    assert active_slots(project) == []
