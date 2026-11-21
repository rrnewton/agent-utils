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
    checkout,
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


@pytest.mark.load_quarantine  # #2065 hot-host-census-tests: remover reaches deletion through live censuses inside a wall-clock wait
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


def _add_containing_remote_branches(target: Path, remote: Path) -> None:
    """Give the checkout 100 remote branches containing its HEAD (#2041).

    That pushes its `containing_remote_refs` inventory past the 64-ref
    inline limit, so the finish journal payload sidecars it (#1757).
    """

    import subprocess

    def run(cwd: Path, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            list(args), cwd=cwd, check=False, capture_output=True, text=True, timeout=120
        )

    head = run(target, "git", "rev-parse", "HEAD").stdout.strip()
    assert head
    for index in range(100):
        made = run(remote, "git", "update-ref", f"refs/heads/probe-{index}", head)
        assert made.returncode == 0, made.stderr
    fetched = run(target, "git", "fetch", "-q", "origin")
    assert fetched.returncode == 0, fetched.stderr


@pytest.mark.load_quarantine  # #2065 hot-host-census-tests: deferred remove proc scan exceeds its 60s budget under host churn
def test_deferred_remove_with_sidecarred_refs_completes(tmp_path: Path) -> None:
    """A deferred remove survives its own sidecarred journal (#2041).

    With more than 64 containing remote refs, the finish journal
    payload carries sidecar keys in memory while the loaded journal is
    read back expanded; the identity compare must still recognise the
    same journal. On the old code the removal refuses "deferred finish
    journal changed before its out-of-lock deletion" with no other
    writer involved.
    """

    project, _repository, remote = make_project(tmp_path)
    assert create(project).returncode == 0
    assert finish(project).returncode == 0
    mark_owner_dead(project)
    set_liveness(project, "dead")
    _add_containing_remote_branches(checkout(project), remote)
    assert _remove_agent_in_process(project) == 0
    assert active_slots(project) == []


def test_private_finish_compare_accepts_sidecarred_journal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The private finish reload accepts its own sidecarred journal (#2041).

    Same asymmetry, at the private compare site: the payload builder
    stamps sidecar keys into the in-memory journal, the loaded journal
    comes back expanded, and the reload must still recognise it.
    """

    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    config = wrkslots._load_config(str(project), "testhost")
    record = wrkslots._load_active(config).slots[0]
    refs = [f"refs/remotes/origin/probe-{index}" for index in range(100)]
    payload = wrkslots._finish_journal_payload(
        config,
        record,
        finished_at="2026-11-21T00:00:00-05:00",
        removed=(),
        phase="prepared",
        finish=wrkslots._FinishContext(
            mode="remove",
            actor="coordinator",
            salvage=(
                {
                    "checkout": record.checkouts[0].name,
                    "containing_remote_refs": refs,
                },
            ),
        ),
    )
    path = wrkslots._journal_path(config)
    wrkslots._write_journal(config, payload, journal_path=path)
    prepared = wrkslots._PreparedPrivateFinish(record, payload, path)
    monkeypatch.setattr(
        wrkslots,
        "_assert_validate_batch_seal_recovery_kind",
        lambda _config, _kind: (record,),
    )
    loaded = wrkslots._load_prepared_private_finish(config, prepared)
    assert loaded["slot"] == record.slot


def test_private_finish_compare_refuses_a_substituted_journal(
    tmp_path: Path,
) -> None:
    """A different journal at the same path still refuses (#2041).

    The canonical compare must not become a rubber stamp: a journal
    whose phase was substituted after the prepare is another journal,
    sidecars or not.
    """

    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    config = wrkslots._load_config(str(project), "testhost")
    record = wrkslots._load_active(config).slots[0]
    refs = [f"refs/remotes/origin/probe-{index}" for index in range(100)]
    payload = wrkslots._finish_journal_payload(
        config,
        record,
        finished_at="2026-11-21T00:00:00-05:00",
        removed=(),
        phase="prepared",
        finish=wrkslots._FinishContext(
            mode="remove",
            actor="coordinator",
            salvage=(
                {
                    "checkout": record.checkouts[0].name,
                    "containing_remote_refs": refs,
                },
            ),
        ),
    )
    path = wrkslots._journal_path(config)
    wrkslots._write_journal(config, payload, journal_path=path)
    substituted = dict(payload)
    substituted["phase"] = "fenced"
    prepared = wrkslots._PreparedPrivateFinish(record, substituted, path)
    with pytest.raises(wrkslots.Refusal, match="private finish journal changed"):
        wrkslots._load_prepared_private_finish(config, prepared)


def test_private_finish_compare_fails_closed_without_sidecar(
    tmp_path: Path,
) -> None:
    """A missing sidecar at compare time refuses, never equals (#2041).

    The canonical compare expands both sides; if the sidecar file a
    journal points at is gone, expansion refuses and the reload must
    not treat the journals as the same. The direct compare call makes
    this test sensitive to a compare that always answers equal or an
    expansion that treats a missing sidecar as nothing: the reload on
    its own never reaches the compare, because journal loading expands
    sidecars while validating shape and refuses first. Both faults
    leave the direct call returning ``True`` instead of raising.
    """

    import json

    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    config = wrkslots._load_config(str(project), "testhost")
    record = wrkslots._load_active(config).slots[0]
    refs = [f"refs/remotes/origin/probe-{index}" for index in range(100)]
    payload = wrkslots._finish_journal_payload(
        config,
        record,
        finished_at="2026-11-21T00:00:00-05:00",
        removed=(),
        phase="prepared",
        finish=wrkslots._FinishContext(
            mode="remove",
            actor="coordinator",
            salvage=(
                {
                    "checkout": record.checkouts[0].name,
                    "containing_remote_refs": refs,
                },
            ),
        ),
    )
    path = wrkslots._journal_path(config)
    wrkslots._write_journal(config, payload, journal_path=path)
    sidecars = list((config.control / "containing-refs").glob("*.json"))
    assert sidecars, "the payload never sidecarred its inventory"
    shrunk = json.loads(path.read_text(encoding="utf-8"))
    for sidecar in sidecars:
        sidecar.unlink()
    with pytest.raises(wrkslots.StateError, match="sidecar"):
        wrkslots._finish_journals_equal(config, shrunk, payload)
    prepared = wrkslots._PreparedPrivateFinish(record, payload, path)
    with pytest.raises((wrkslots.Refusal, wrkslots.StateError)):
        wrkslots._load_prepared_private_finish(config, prepared)


@pytest.mark.load_quarantine  # #2065 hot-host-census-tests: remover reaches completion through live censuses inside a wall-clock wait
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


@pytest.mark.load_quarantine  # #2065 hot-host-census-tests: batch reaches group scan through live censuses inside a wall-clock wait
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


@pytest.mark.load_quarantine  # #2065 hot-host-census-tests: interrupted-batch setup reaches its seal through live censuses
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


@pytest.mark.load_quarantine  # #2065 hot-host-census-tests: grouped batch remove proc scans exceed their budget under host churn
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
