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

import json
import os
from collections.abc import Mapping, Sequence
from pathlib import Path

import pytest

from wrkslots import cli as wrkslots
from wrkslots.tests.test_lifecycle import (
    active_slots,
    checkout,
    create,
    expire_heartbeat,
    make_project,
    mark_owner_dead,
    set_liveness,
    stub_validate_batch_censuses,
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
) -> Path:
    handle = project / "ignored" / "validate" / "runs" / (
        RUN_UNIT.removesuffix(".service") + ".json"
    )
    handle.parent.mkdir(parents=True, exist_ok=True)
    value: dict[str, object] = {"checkout": str(tree), "unit": RUN_UNIT}
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
