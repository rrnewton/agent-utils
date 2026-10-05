"""A validation row's liveness is answered by its run, not by an agent name.

Validation launchers register a run label such as ``validate-<slot>`` as the
row's agent.  No process carries that label, so the configured agent-liveness
command cannot answer for it: on 2026-10-05 it reported every one of 54
validation rows on one host ``unverifiable``, and ``remove --validate-complete``
refused every completed checkout whose launcher had already exited.  These
tests pin the replacement authority -- retained run handles, their exact
process generations and units, and user-systemd state -- in both directions:
removal succeeds when the run is provably over even though the agent-name
command is unverifiable, and it refuses whenever the run may still be using the
checkout even though the agent-name command says dead.
"""

from __future__ import annotations

import contextlib
import dataclasses
import json
import os
import subprocess
import uuid
from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path

import pytest

from wrkslots import cli as wrkslots
from wrkslots.tests.test_lifecycle import (
    active_slots,
    allow_test_host_for_absent_validate_recovery,
    checkout,
    create,
    expire_heartbeat,
    interrupt_validate_batch,
    make_project,
    mark_owner_dead,
    prepare_absent_validate_row,
    prepare_dead_validate_slots,
    run_absent_validate_recovery,
    set_liveness,
    stub_validate_batch_censuses,
    write_absent_validate_input,
)


# These tests supply the host's process and user-systemd evidence themselves,
# below the seam that the suite's idle-host default replaces.
pytestmark = pytest.mark.validation_run_evidence

RUN_UNIT = "validate-run-0001.service"


def _unit(**overrides: str) -> dict[str, str]:
    value = {
        "Id": RUN_UNIT,
        "LoadState": "loaded",
        "ActiveState": "inactive",
        "SubState": "dead",
        "MainPID": "0",
        "ControlGroup": f"/user.slice/app.slice/{RUN_UNIT}",
        "WorkingDirectory": "",
        "ExecStart": "",
        "Environment": "",
        "PendingJob": "no",
    }
    value.update(overrides)
    return value


def _prepare(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    *,
    agent_liveness: str,
    units: Sequence[Mapping[str, str]] = (),
    processes: Sequence[wrkslots._AbsentProcessObservation] = (),
) -> tuple[Path, Path]:
    """Create one completed validation row whose launcher has exited."""

    project, _repository, _remote = make_project(tmp_path)
    made = create(
        project,
        agent="validate-slot01",
        slot_type="validate",
        branch=None,
    )
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    set_liveness(project, agent_liveness)
    stub_validate_batch_censuses(monkeypatch)
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_a, **_k: None)
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: tuple(units))
    monkeypatch.setattr(
        wrkslots,
        "_absent_validate_process_snapshot",
        lambda **_kwargs: tuple(processes),
    )
    return project, checkout(project, slot_type="validate")


def _write_run_handle(
    project: Path,
    tree: Path,
    *,
    process_identity: Mapping[str, object] | None = None,
    unit: str = RUN_UNIT,
) -> Path:
    handle = project / "ignored" / "validate" / "runs" / (
        unit.removesuffix(".service") + ".json"
    )
    handle.parent.mkdir(parents=True, exist_ok=True)
    value: dict[str, object] = {"checkout": str(tree), "unit": unit}
    if process_identity is not None:
        value["process_identity"] = dict(process_identity)
    handle.write_text(json.dumps(value), encoding="utf-8")
    return handle


def _remove_completed(project: Path) -> int:
    return wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove",
            "slot01",
            "--validate-complete",
            "--coordinator-authorized",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )


def _assert_retained(project: Path, tree: Path) -> None:
    assert tree.is_dir()
    assert len(active_slots(project)) == 1


def test_completed_validation_removal_does_not_ask_agent_liveness_about_a_run_label(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """The incident: the agent-name command cannot answer for a run label."""

    project, tree = _prepare(
        tmp_path,
        monkeypatch,
        agent_liveness="unverifiable",
        units=(_unit(),),
    )
    _write_run_handle(project, tree)

    removed = _remove_completed(project)

    assert removed == 0, capsys.readouterr().err
    assert not tree.exists()
    assert active_slots(project) == []


def test_completed_validation_removal_refuses_while_the_run_unit_is_active(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, tree = _prepare(
        tmp_path,
        monkeypatch,
        agent_liveness="dead",
        units=(_unit(ActiveState="active", SubState="running"),),
    )
    _write_run_handle(project, tree)

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert "validation-run authority reports the run may still use slot slot01" in error
    assert f"retained validation unit {RUN_UNIT} may still use row slot01" in error
    _assert_retained(project, tree)


def test_completed_validation_removal_refuses_while_the_run_job_is_queued(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, tree = _prepare(
        tmp_path,
        monkeypatch,
        agent_liveness="dead",
        units=(_unit(PendingJob="yes"),),
    )
    _write_run_handle(project, tree)

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert f"retained validation unit {RUN_UNIT} may still use row slot01" in error
    _assert_retained(project, tree)


def test_completed_validation_removal_refuses_a_live_run_process_generation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, tree = _prepare(
        tmp_path, monkeypatch, agent_liveness="dead", units=(_unit(),)
    )
    start_ticks = wrkslots._process_start_ticks(Path("/proc") / str(os.getpid()))
    assert start_ticks is not None
    _write_run_handle(
        project,
        tree,
        process_identity={
            "pid": os.getpid(),
            "start_ticks": start_ticks,
            "boot_id": wrkslots._boot_id(Path("/proc")),
        },
    )

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert f"has live exact process generation {os.getpid()} for row slot01" in error
    _assert_retained(project, tree)


def test_completed_validation_removal_refuses_a_process_left_in_the_run_cgroup(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    straggler = wrkslots._AbsentProcessObservation(
        pid=os.getpid() + 100_000,
        start_ticks=17,
        cgroup_path=f"/user.slice/app.slice/{RUN_UNIT}/payload",
        mount_namespace="mnt:[test]",
    )
    project, tree = _prepare(
        tmp_path,
        monkeypatch,
        agent_liveness="dead",
        units=(_unit(),),
        processes=(straggler,),
    )
    _write_run_handle(project, tree)

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert f"retained validation unit {RUN_UNIT} still has a live cgroup process" in error
    _assert_retained(project, tree)


def test_completed_validation_removal_refuses_an_active_unit_naming_the_checkout(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """A run with no retained handle is still visible through its unit."""

    project, tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")
    unrecorded = _unit(
        Id="unrecorded-run.service",
        ActiveState="active",
        SubState="running",
        WorkingDirectory=str(tree),
    )
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: (unrecorded,))

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert "user-systemd unit unrecorded-run.service names validation row slot01" in error
    _assert_retained(project, tree)


def test_completed_validation_removal_refuses_unreadable_run_evidence(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")

    def unreadable() -> tuple[Mapping[str, str], ...]:
        raise wrkslots.Refusal("cannot enumerate user-systemd state: no user bus")

    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", unreadable)

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert "validation-run authority is unverifiable for slot slot01" in error
    assert "no user bus" in error
    _assert_retained(project, tree)


def test_audit_judges_a_validation_row_by_its_run(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """Audit and remove ask the same authority, so they cannot disagree."""

    project, tree = _prepare(
        tmp_path, monkeypatch, agent_liveness="unverifiable", units=(_unit(),)
    )
    _write_run_handle(project, tree)
    expire_heartbeat(project)

    def audit() -> str:
        code = wrkslots.main(
            ["--project-root", str(project), "audit", "--format", "json"]
        )
        captured = capsys.readouterr()
        assert code == 0, captured.err
        return captured.out

    finished = json.loads(audit())
    phases = {phase["name"]: phase for phase in finished["metrics"]["phases"]}
    assert phases["liveness"]["work"] == {
        "batch_invocations": 0,
        "legacy_invocations": 0,
        "subjects": 0,
        "validation_run_subjects": 1,
    }
    row = finished["slots"][0]
    assert row["liveness_state"] == "dead"
    assert row["verdict"] == "DELETABLE", row["reasons"]

    monkeypatch.setattr(
        wrkslots,
        "_user_systemd_snapshot",
        lambda: (_unit(ActiveState="active", SubState="running"),),
    )
    running = json.loads(audit())
    row = running["slots"][0]
    assert row["liveness_state"] == "alive"
    assert row["verdict"] == "BLOCKED"
    assert any(
        f"retained validation unit {RUN_UNIT} may still use row slot01" in reason
        for reason in row["reasons"]
    )


def _live_identity() -> dict[str, object]:
    start_ticks = wrkslots._process_start_ticks(Path("/proc") / str(os.getpid()))
    assert start_ticks is not None
    return {
        "pid": os.getpid(),
        "start_ticks": start_ticks,
        "boot_id": wrkslots._boot_id(Path("/proc")),
    }


@pytest.mark.parametrize(
    ("duplicated", "trailer"),
    [
        # A later null identity would hide the live process generation.
        ("process_identity", '"process_identity": null'),
        # A later checkout would move the handle off this row entirely.
        ("checkout", '"checkout": "/elsewhere/unrelated-checkout"'),
    ],
)
def test_completed_validation_removal_refuses_a_handle_with_a_duplicate_field(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    duplicated: str,
    trailer: str,
) -> None:
    """A JSON object with two values for one field has no single meaning.

    A permissive decoder keeps the last value.  Here the first values say the
    run's process is live on this row's checkout, and the appended duplicate
    would make the row look dead.  The handle is unreadable evidence instead.
    """

    project, tree = _prepare(
        tmp_path, monkeypatch, agent_liveness="dead", units=(_unit(),)
    )
    handle = _write_run_handle(project, tree, process_identity=_live_identity())
    text = handle.read_text(encoding="utf-8")
    assert text.endswith("}")
    handle.write_text(f"{text[:-1]}, {trailer}}}", encoding="utf-8")

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert "validation-run authority is unverifiable for slot slot01" in error
    assert f"duplicate key {duplicated!r}" in error
    _assert_retained(project, tree)


def _slot_directory(project: Path) -> Path:
    config = wrkslots._load_config(str(project), "testhost")
    return wrkslots._slot_directory(config, "slot01", "validate")


@pytest.mark.parametrize(
    ("property_name", "spelling"),
    [
        ("ExecStart", "{parent}/./{name}"),
        ("ExecStart", "--checkout={parent}//{name}/product"),
        ("WorkingDirectory", "{parent}/elsewhere/../{name}"),
        ("Environment", "RUN_ROOT={parent}/./{name}/./product"),
        ("RequiresMountsFor", "{parent}/{name}/"),
    ],
)
def test_completed_validation_removal_refuses_a_queued_unit_naming_an_alias_of_the_row(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    property_name: str,
    spelling: str,
) -> None:
    """A queued job names the row through a different spelling of its path.

    The job has not started, so no process or handle shows it yet.  Its unit
    evidence names the row's directory with ``/./``, ``//`` or ``x/..``
    steps; each spelling is the same path, so the row may still be used.
    """

    project, tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")
    slot_directory = _slot_directory(project)
    alias = spelling.format(parent=slot_directory.parent, name=slot_directory.name)
    queued = _unit(Id="queued-run.service", PendingJob="yes", **{property_name: alias})
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: (queued,))

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert "validation-run authority reports the run may still use slot slot01" in error
    assert "user-systemd unit queued-run.service names validation row slot01" in error
    _assert_retained(project, tree)


@pytest.mark.parametrize("sibling", ["slot010", "slot01-old", "slot01.bak", "slot01_next"])
def test_completed_validation_removal_ignores_an_active_unit_naming_a_sibling_row(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    sibling: str,
) -> None:
    """``slot010`` begins with the text ``slot01`` but is a different path."""

    project, tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")
    other = _slot_directory(project).parent / sibling
    running = _unit(
        Id="sibling-run.service",
        ActiveState="active",
        SubState="running",
        WorkingDirectory=str(other),
        ExecStart=f"/usr/bin/env\n--checkout={other}/product",
        Environment=f"RUN_ROOT={other}",
    )
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: (running,))

    removed = _remove_completed(project)

    assert removed == 0, capsys.readouterr().err
    assert not tree.exists()
    assert active_slots(project) == []


def test_unit_path_evidence_keeps_each_normalization_stage(tmp_path: Path) -> None:
    """One property string can hold several paths and other words.

    The ``x/..`` later in this shell command would remove the tail of the row
    path if only the fully reduced text were compared; the stage after the
    first ``..`` step still shows the row path whole.
    """

    row = "/project/worktrees/validate/slot01"
    command = "cd /project/worktrees/tmp/../validate/slot01 && ls build/../out"
    evidence = wrkslots._lexical_path_evidence(command)
    assert wrkslots._evidence_names_path(evidence, row)
    assert not wrkslots._evidence_names_path(
        wrkslots._lexical_path_evidence("/project/worktrees/validate/slot010"), row
    )
    assert wrkslots._evidence_names_path(
        wrkslots._lexical_path_evidence(f"PATH=/usr/bin:{row}:/bin"), row
    )

    steps = "/a/.." * (wrkslots._PARENT_DIRECTORY_STEPS_LIMIT + 1)
    with pytest.raises(wrkslots.Refusal, match="parent-directory steps"):
        wrkslots._lexical_path_evidence(f"{steps}{row}")

    real = tmp_path / "real"
    (real / "worktrees").mkdir(parents=True)
    link = tmp_path / "link"
    link.symlink_to(real)
    spellings = wrkslots._row_path_spellings(link / "worktrees" / "validate" / "slot01")
    resolved = wrkslots._lexical_path_evidence(f"{real}/worktrees/validate/slot01")
    assert any(wrkslots._evidence_names_path(resolved, spelling) for spelling in spellings)


UNREADABLE_PID = 4_000_017


def _write_unreadable_generation_handle(
    project: Path, tree: Path, monkeypatch: pytest.MonkeyPatch, evidence: str
) -> None:
    """Record a run process whose generation or boot cannot be read."""

    _write_run_handle(
        project,
        tree,
        process_identity={
            "pid": UNREADABLE_PID,
            "start_ticks": 17,
            "boot_id": wrkslots._boot_id(Path("/proc")),
        },
    )
    if evidence == "boot-id":
        def boot_id(_proc_root: Path) -> str:
            raise wrkslots.Refusal("cannot read the machine boot id: test")

        # Only the run authority reads the boot id after this point in the
        # direct call below.
        monkeypatch.setattr(wrkslots, "_boot_id", boot_id)
        return
    real_start_ticks = wrkslots._process_start_ticks

    def start_ticks(pid_dir: Path) -> int | None:
        if pid_dir.name == str(UNREADABLE_PID):
            raise wrkslots.Refusal(
                f"process generation is indeterminate because {pid_dir / 'stat'} "
                "is unreadable: test"
            )
        return real_start_ticks(pid_dir)

    monkeypatch.setattr(wrkslots, "_process_start_ticks", start_ticks)


@pytest.mark.parametrize(
    ("evidence", "detail"),
    [
        ("boot-id", "cannot read the machine boot id"),
        ("pid-generation", "process generation is indeterminate"),
    ],
)
def test_unreadable_run_process_evidence_is_unverifiable_not_alive(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    evidence: str,
    detail: str,
) -> None:
    """Evidence that cannot be read is not evidence of a run.

    ``alive`` tells the operator to stop the run's unit, which is the wrong
    remedy when nothing shows a run at all.
    """

    project, tree = _prepare(
        tmp_path, monkeypatch, agent_liveness="dead", units=(_unit(),)
    )
    config = wrkslots._load_config(str(project), "testhost")
    records = wrkslots._load_active(config).slots
    _write_unreadable_generation_handle(project, tree, monkeypatch, evidence)

    states = wrkslots._validation_run_liveness_states(config, records)

    assert len(states) == 1
    state, message = next(iter(states.values()))
    assert state == "unverifiable", message
    assert detail in message


def test_completed_validation_removal_names_unreadable_run_evidence_as_unverifiable(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, tree = _prepare(
        tmp_path, monkeypatch, agent_liveness="dead", units=(_unit(),)
    )
    _write_unreadable_generation_handle(project, tree, monkeypatch, "pid-generation")

    removed = _remove_completed(project)

    error = capsys.readouterr().err
    assert removed != 0
    assert "validation-run authority is unverifiable for slot slot01" in error
    assert "process generation is indeterminate" in error
    assert "stop its unit" not in error
    _assert_retained(project, tree)


def _run_child(cgroup: str, *, pid: int = 4_000_029) -> wrkslots._AbsentProcessObservation:
    return wrkslots._AbsentProcessObservation(
        pid=pid, start_ticks=31, cgroup_path=cgroup, mount_namespace="mnt:[test]"
    )


RUN_CGROUP = f"/user.slice/app.slice/{RUN_UNIT}"


def _judge_with_process_tables(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    before: Sequence[wrkslots._AbsentProcessObservation],
    after: Sequence[wrkslots._AbsentProcessObservation],
) -> tuple[str, str]:
    """Judge one row whose process table changes during the unit enumeration."""

    project, tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")
    _write_run_handle(project, tree)
    enumerated = False

    def units() -> tuple[Mapping[str, str], ...]:
        nonlocal enumerated
        enumerated = True
        # The run started and finished during this enumeration.
        return (_unit(),)

    def processes(**_kwargs: object) -> tuple[wrkslots._AbsentProcessObservation, ...]:
        return tuple(after if enumerated else before)

    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", units)
    monkeypatch.setattr(wrkslots, "_absent_validate_process_snapshot", processes)
    config = wrkslots._load_config(str(project), "testhost")
    states = wrkslots._validation_run_liveness_states(
        config, wrkslots._load_active(config).slots
    )
    assert enumerated
    assert len(states) == 1
    return next(iter(states.values()))


def test_a_run_child_that_appears_during_the_unit_enumeration_is_alive(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The unit reads inactive and unqueued, but its child is still running.

    The process table read before the enumeration is empty.  Judging the row
    from it alone reports ``dead``, and the later path census does not look
    at the run's control group, so a child holding no path escapes both.
    """

    state, message = _judge_with_process_tables(
        tmp_path, monkeypatch, before=(), after=(_run_child(f"{RUN_CGROUP}/payload"),)
    )

    assert state == "alive", message
    assert f"retained validation unit {RUN_UNIT} still has a live cgroup process" in message


def test_a_run_child_that_leaves_the_run_cgroup_during_the_enumeration_is_alive(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The same exact process generation keeps the run's control group."""

    state, message = _judge_with_process_tables(
        tmp_path,
        monkeypatch,
        before=(_run_child(f"{RUN_CGROUP}/payload"),),
        after=(_run_child("/user.slice/app.slice/elsewhere.scope"),),
    )

    assert state == "alive", message
    assert f"retained validation unit {RUN_UNIT} still has a live cgroup process" in message


def test_a_run_child_that_exited_during_the_enumeration_is_not_alive(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A generation absent from the later table has exited."""

    state, message = _judge_with_process_tables(
        tmp_path, monkeypatch, before=(_run_child(f"{RUN_CGROUP}/payload"),), after=()
    )

    assert state == "dead", message


def test_absent_row_recovery_sees_a_run_child_that_appears_during_the_unit_enumeration(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """recover-absent-validate-rows judges the same evidence the same way.

    Its census reads the process table, then the unit enumeration runs.  A
    run that starts and finishes inside that enumeration leaves its unit
    inactive and unqueued and its child in the unit's control group, which
    only a table read after the enumeration shows.
    """

    project, repository, _remote = make_project(tmp_path)
    record = prepare_absent_validate_row(project, repository, slot="gone", agent="validate-a")
    config = wrkslots._load_config(str(project), "testhost")
    _write_run_handle(
        project, wrkslots._stored_path(config, record.checkouts[0].path, "checkout")
    )
    input_path = write_absent_validate_input(project, [record])
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    enumerated = False

    def units() -> tuple[Mapping[str, str], ...]:
        nonlocal enumerated
        enumerated = True
        return (_unit(),)

    def processes(**_kwargs: object) -> tuple[wrkslots._AbsentProcessObservation, ...]:
        return (_run_child(f"{RUN_CGROUP}/payload"),) if enumerated else ()

    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", units)
    monkeypatch.setattr(wrkslots, "_absent_validate_process_snapshot", processes)

    assert run_absent_validate_recovery(project, input_path, apply=False) == 3
    assert enumerated
    error = capsys.readouterr().err
    assert f"retained validation unit {RUN_UNIT} still has a live cgroup process" in error


# The tests below leave the host-evidence seam in place: the retained handles,
# this host's real process table (read on both sides of the unit enumeration)
# and the boot id are all read for real, and the run's process is a real child.
# Only the user-systemd enumeration is supplied, because some hosts that run
# this suite have no user bus and a busy host's real unit population changes
# under the enumeration; the enumeration's own parsing has its own tests.  The
# run's unit has a unique name so no real unit or process on the host matches
# it.  Each real process-table read is recorded, so a test can show that the
# judgment read this host's table and saw (or no longer saw) the run's process.


@dataclass(frozen=True)
class _RealHost:
    project: Path
    slot_path: Path
    tree: Path
    unit: str
    tables: list[tuple[wrkslots._AbsentProcessObservation, ...]]

    def tables_with(self, generation: tuple[int, int]) -> int:
        return sum(
            any((process.pid, process.start_ticks) == generation for process in table)
            for table in self.tables
        )


def _prepare_real_host(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> _RealHost:
    """One validation row whose recorded owner has exited, on the real host."""

    project, _repository, _remote = make_project(tmp_path)
    slot_path = prepare_dead_validate_slots(project, ("slot01",))["slot01"]
    tree = checkout(project, slot="slot01", slot_type="validate")
    unit = f"wrkslots-test-{uuid.uuid4().hex}.service"
    stub_validate_batch_censuses(monkeypatch)
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_a, **_k: None)
    monkeypatch.setattr(
        wrkslots,
        "_user_systemd_snapshot",
        lambda: (_unit(Id=unit, ControlGroup=f"/user.slice/app.slice/{unit}"),),
    )
    # The suite's idle-host default replaces this seam; the marker keeps it.
    assert wrkslots._validation_run_host_evidence.__module__ == wrkslots.__name__
    tables: list[tuple[wrkslots._AbsentProcessObservation, ...]] = []
    real_snapshot = wrkslots._absent_validate_process_snapshot

    def recorded_snapshot(
        proc_root: Path = Path("/proc"), *, include_owner_cgroups: bool = True
    ) -> tuple[wrkslots._AbsentProcessObservation, ...]:
        table = real_snapshot(proc_root, include_owner_cgroups=include_owner_cgroups)
        tables.append(table)
        return table

    monkeypatch.setattr(wrkslots, "_absent_validate_process_snapshot", recorded_snapshot)
    return _RealHost(project, slot_path, tree, unit, tables)


@dataclass(frozen=True)
class _RunProcess:
    child: subprocess.Popen[bytes]
    generation: tuple[int, int]


@contextlib.contextmanager
def _real_run_process(host: _RealHost) -> Iterator[_RunProcess]:
    """Record a live child as the run's process generation in its handle."""

    child = subprocess.Popen(
        ["sleep", "300"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        start_ticks = wrkslots._process_start_ticks(Path("/proc") / str(child.pid))
        assert start_ticks is not None
        _write_run_handle(
            host.project,
            host.tree,
            unit=host.unit,
            process_identity={
                "pid": child.pid,
                "start_ticks": start_ticks,
                "boot_id": wrkslots._boot_id(Path("/proc")),
            },
        )
        yield _RunProcess(child, (child.pid, start_ticks))
    finally:
        if child.poll() is None:
            child.kill()
        child.wait()


def _end_run(host: _RealHost, run: _RunProcess) -> None:
    """The run's process exits and is reaped, so its generation is gone."""

    run.child.kill()
    run.child.wait()
    host.tables.clear()


def _assert_judged_live_run(host: _RealHost, run: _RunProcess, text: str) -> None:
    """The refusal named the run's process, and the real table showed it."""

    assert f"has live exact process generation {run.child.pid} for row slot01" in text
    # Read on both sides of the unit enumeration.
    assert host.tables_with(run.generation) >= 2


def _assert_judged_ended_run(host: _RealHost, run: _RunProcess) -> None:
    assert len(host.tables) >= 2
    assert host.tables_with(run.generation) == 0


def test_real_host_completed_removal_waits_for_the_run_process(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    host = _prepare_real_host(tmp_path, monkeypatch)

    with _real_run_process(host) as run:
        assert _remove_completed(host.project) == 3
        error = capsys.readouterr().err
        assert "validation-run authority reports the run may still use slot slot01" in error
        _assert_judged_live_run(host, run, error)
        _assert_retained(host.project, host.tree)

        _end_run(host, run)
        assert _remove_completed(host.project) == 0, capsys.readouterr().err
        _assert_judged_ended_run(host, run)

    assert not host.slot_path.exists()
    assert active_slots(host.project) == []


def _remove_batch(project: Path) -> int:
    return wrkslots.main(
        [
            "--project-root",
            str(project),
            "remove-validate-batch",
            "--coordinator-pid",
            str(os.getpid()),
            "--slot",
            "slot01=1",
        ]
    )


def test_real_host_batch_seal_waits_for_the_run_process(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    host = _prepare_real_host(tmp_path, monkeypatch)
    config = wrkslots._load_config(str(host.project), "testhost")

    with _real_run_process(host) as run:
        # A batch retains each refused slot and exits 1.
        assert _remove_batch(host.project) == 1
        output = capsys.readouterr().out
        assert (
            "RETAINED: slot01 reason=validation-run authority reports the run may "
            "still use slot slot01"
        ) in output
        _assert_judged_live_run(host, run, output)
        assert not wrkslots._validate_batch_seal_journal_path(config).exists()
        _assert_retained(host.project, host.tree)

        _end_run(host, run)
        assert _remove_batch(host.project) == 0, capsys.readouterr()
        _assert_judged_ended_run(host, run)

    assert not host.slot_path.exists()
    assert active_slots(host.project) == []


def test_real_host_finish_recovery_waits_for_the_run_process(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    host = _prepare_real_host(tmp_path, monkeypatch)
    interrupt_validate_batch(host.project, monkeypatch, "after-finish-journal", ("slot01",))
    config = wrkslots._load_config(str(host.project), "testhost")
    finish_path = wrkslots._journal_path(config)
    assert finish_path.exists()
    recover = [
        "--project-root",
        str(host.project),
        "recover",
        "--coordinator-pid",
        str(os.getpid()),
    ]

    # A run that starts on the checkout after the batch journaled its finish.
    with _real_run_process(host) as run:
        host.tables.clear()
        assert wrkslots.main(recover) == 3
        error = capsys.readouterr().err
        _assert_judged_live_run(host, run, error)
        assert finish_path.exists()
        _assert_retained(host.project, host.tree)

        _end_run(host, run)
        assert wrkslots.main(recover) == 0, capsys.readouterr().err
        _assert_judged_ended_run(host, run)

    assert not host.slot_path.exists()
    assert not finish_path.exists()
    assert active_slots(host.project) == []


@pytest.mark.parametrize(
    ("lease_host", "expected"), [("foreign-host", "unverifiable"), (None, "dead")]
)
def test_an_ownerless_row_is_judged_local_by_its_coordinator_lease(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    lease_host: str | None,
    expected: str,
) -> None:
    """An ownerless row's lease names the host that registered it.

    With no owner, a host check that looks only at the owner accepts a row
    registered on another host and judges it ``dead`` from this host's empty
    evidence.  The same row is local when its lease names this host.
    """

    project, _tree = _prepare(tmp_path, monkeypatch, agent_liveness="dead")
    config = wrkslots._load_config(str(project), "testhost")
    state = wrkslots._load_active(config)
    record = state.slots[0]
    lease = record.coordinator_lease
    if lease_host is not None:
        lease = dataclasses.replace(lease, host_id=lease_host)
    ownerless = dataclasses.replace(record, owner=None, coordinator_lease=lease)
    wrkslots._write_active_state(
        config,
        wrkslots._replace_record(state, ownerless),
        action="test-ownerless",
        slot=record.slot,
    )

    states = wrkslots._validation_run_liveness_states(
        config, wrkslots._load_active(config).slots
    )

    state_name, message = states[("testhost", "slot01", 1)]
    assert state_name == expected, message
    if expected == "unverifiable":
        assert "belongs to stable host foreign-host" in message
